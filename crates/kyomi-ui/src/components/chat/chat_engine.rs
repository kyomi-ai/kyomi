// SPDX-License-Identifier: AGPL-3.0-or-later

//! Unified chat engine — reactive state container for all chat UIs.
//!
//! `ChatEngine` is NOT a Leptos component. It's a struct with reactive signals
//! and methods that consumers create in their component bodies. It projects a
//! retained ChatRunStore session so that both `CopilotChat` (copilot sidebars) and
//! `chat_page.rs` (main chat page) can use the same engine with different configs.
//!
//! ## Session modes
//!
//! - **Ephemeral**: Creates a session on activate, deletes on deactivate. For
//!   copilot sidebars that spin up temporary sessions tied to a context.
//! - **External**: Session ID managed by the caller (URL-driven). For the main
//!   chat page where the session lifecycle is controlled externally.
//!
//! ## Filtering
//!
//! - If `context_type` is Some: filters WS events by context_type AND session_id
//!   (copilot pattern).
//! - If `context_type` is None: filters by session_id only (main chat pattern).
//! - **Default-deny (KYO-494)**: the session_id check never admits an event
//!   when either side is missing an identity — a `None` engine session_id
//!   (a brand-new, not-yet-created chat) is not "no filter," it's "nothing
//!   matches yet." See `should_handle`'s doc comment below.

use leptos::prelude::*;

use super::ChatState;
use super::chat_run_store::{ChatRun, ChatRunStore};
#[cfg(target_arch = "wasm32")]
use super::websocket_client::WebSocketContext;
use super::{ChatStateMachine, ThinkingManager};
use crate::server_fns::chat::ChatMessageItem;
use crate::server_fns::copilot::{
    create_copilot_session, delete_copilot_session, send_copilot_message,
    CopilotMessageRequest,
};

// ─── Session mode ──────────────────────────────────────────────────────────

/// Controls how the engine manages sessions.
pub enum SessionMode {
    /// Ephemeral: create on activate, delete on deactivate. For copilot sidebars.
    Ephemeral {
        /// The context type for session creation (e.g. "dashboard_copilot").
        context_type: String,
        /// When true, the session is created. When false, it is deleted.
        /// If `None`, the session is created immediately on mount.
        active: Option<Signal<bool>>,
    },
    /// External: session_id managed by the caller (URL-driven). For main chat page.
    External {
        /// Caller-managed session ID signal.
        session_id: Signal<Option<String>>,
    },
}

/// A hook invoked immediately before a copilot message is dispatched to the
/// server, so the caller can commit any pending buffer first — the copilot
/// always edits the *saved* document, never an unsaved one (KYO-536).
/// Returns `true` to proceed with sending, `false` to abort (e.g. the save
/// itself failed) — [`ChatEngine::send`] surfaces `false` as its own send
/// error via the existing error banner rather than silently dispatching a
/// copilot message on top of a failed save.
///
/// `Arc<dyn .. + Send + Sync>` and a `Send` future, not `Rc`/no bound: this
/// value lives on [`ChatEngine`], which is captured by `leptos::prelude::
/// Callback` closures (`Callback::new` requires `Fn(In) -> Out + Send +
/// Sync + 'static` unconditionally, for SSR compatibility, even though the
/// client itself is single-threaded WASM) — so every field `ChatEngine`
/// carries must satisfy that bound too.
pub type BeforeSendHook = std::sync::Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> + Send + Sync,
>;

// ─── Config ────────────────────────────────────────────────────────────────

/// Configuration for creating a `ChatEngine`.
pub struct ChatEngineConfig {
    /// How the engine manages session lifecycle.
    pub session_mode: SessionMode,
    /// For WS event filtering. Copilot sets this to e.g. "dashboard_copilot".
    /// Main chat leaves it None (filters by session_id only).
    pub context_type: Option<String>,
    /// Custom WS event names to subscribe to (e.g. ["dashboard_update"]).
    pub custom_ws_events: Vec<String>,
    /// Handler for custom WS events. Receives (event_name, data).
    pub on_custom_ws_event: Option<Callback<(String, serde_json::Value)>>,
    /// Content context signal (dashboard markdown, chart YAML, etc.).
    pub context_content: Option<Signal<String>>,
    /// Label for context prefix ("Dashboard Content", "Chart Content", etc.).
    pub context_label: Option<String>,
    /// Id of the dashboard/knowledge document this copilot session is open
    /// against, if any. Threaded through to `send_copilot_message`, and
    /// from there to `ToolContext::document_id` — see that field's doc
    /// comment. `None` for copilot types with no single open document
    /// (chart builder, watch).
    pub document_id: Option<String>,
    /// Viewer state sampled for every send, rather than at session creation.
    pub view_context: Option<Signal<String>>,
    /// Historical previews may be discussed but never mutated.
    pub historical_preview: Option<Signal<bool>>,
    /// See [`BeforeSendHook`]. `None` for copilot types with nothing to
    /// autosave (chart builder, watch).
    pub before_send: Option<BeforeSendHook>,
}

// ─── SendRequest ───────────────────────────────────────────────────────────

/// Data prepared by the engine for a send operation.
///
/// In Ephemeral mode, the engine calls the copilot server function directly.
/// In External mode, consumers use `add_user_message()` to get the optimistic
/// message and handle the server call themselves.
pub struct SendRequest {
    /// The session ID to send to.
    pub session_id: String,
    /// The user's message text.
    pub message: String,
    /// The context type (for copilot sends).
    pub context_type: Option<String>,
    /// The context content with prefix label (for copilot sends).
    pub context: Option<String>,
}

// ─── ChatEngine ────────────────────────────────────────────────────────────

/// Unified reactive state container for chat UIs.
///
/// Projects shared messages, thinking and state for its selected session.
/// Consumers create this in their component body and
/// use the public API to drive the UI.
#[derive(Clone)]
pub struct ChatEngine {
    // Public read signals
    run: RwSignal<ChatRun>,
    store: ChatRunStore,
    view_id: u64,
    generation: RwSignal<u64>,
    // Captured ONCE at construction time (see `messages()` below for why —
    // KYO-781) rather than derived on every accessor call.
    messages_read: Signal<Vec<ChatMessageItem>>,
    chat_state: ChatStateMachine,
    thinking: ThinkingManager,
    session_id: RwSignal<Option<String>>,
    session_id_read: ReadSignal<Option<String>>,

    // Internal state
    has_sent_first: RwSignal<bool>,
    context_type: StoredValue<Option<String>>,
    context_content: Option<Signal<String>>,
    context_label: StoredValue<Option<String>>,
    document_id: Option<String>,
    view_context: Option<Signal<String>>,
    historical_preview: Option<Signal<bool>>,
    before_send: Option<BeforeSendHook>,
}

impl ChatEngine {
    /// Create a view and set up its session lifecycle.
    ///
    /// Must be called inside a Leptos reactive owner (component body or
    /// `Owner::with`). The engine registers effects and cleanup handlers
    /// that are tied to the component lifecycle.
    pub fn new(config: ChatEngineConfig) -> Self {
        let (engine, session_mode) = Self::create_view(config);
        let store = engine.store;
        let session_id = engine.session_id;

        // ── Session lifecycle ──────────────────────────────────────────
        match session_mode {
            SessionMode::Ephemeral {
                context_type: ctx_type,
                active,
            } => {
                let ctx_type_stored = StoredValue::new(ctx_type.clone());
                let active_signal = active;
                let engine_for_session = engine.clone();
                // Guard against concurrent session creation (Effect can fire
                // multiple times before the async create completes).
                let is_creating = RwSignal::new(None::<u64>);

                Effect::new(move || {
                    let generation = store.generation.get();
                    if engine_for_session.generation.get_untracked() != generation {
                        let old_session = session_id.get_untracked();
                        engine_for_session.select_session(None);
                        is_creating.set(None);
                        if let Some(sid) = old_session {
                            leptos::task::spawn_local(async move {
                                let _ = delete_copilot_session(sid).await;
                            });
                        }
                    }
                    let should_be_active = active_signal.is_none_or(|s| s.get());

                    if should_be_active
                        && session_id.get_untracked().is_none()
                        && is_creating.get_untracked().is_none()
                    {
                        // Reset all state for fresh session.
                        engine_for_session.select_session(None);
                        engine_for_session.reset();
                        is_creating.set(Some(generation));

                        let Some(ctx_type) = ctx_type_stored.try_get_value() else {
                            return;
                        };
                        let engine_created = engine_for_session.clone();
                        leptos::task::spawn_local(async move {
                            match create_copilot_session(ctx_type).await {
                                Ok(sid) => {
                                    if is_creating.try_get_untracked() == Some(Some(generation))
                                        && store.generation.try_get_untracked() == Some(generation)
                                        && active_signal.is_none_or(|active| {
                                            active.try_get_untracked() == Some(true)
                                        })
                                    {
                                        engine_created.select_session(Some(sid));
                                    } else {
                                        let _ = delete_copilot_session(sid).await;
                                    }
                                }
                                Err(e) => {
                                    // Guard: the component may have been disposed
                                    // while the async create was in flight.
                                    if is_creating.try_get_untracked() == Some(Some(generation))
                                        && store.generation.try_get_untracked() == Some(generation)
                                    {
                                        engine_created
                                            .chat_state()
                                            .set_error(&format!("Failed to start copilot: {e}"));
                                    }
                                }
                            }
                            if is_creating.try_get_untracked() == Some(Some(generation)) {
                                is_creating.try_set(None);
                            }
                        });
                    } else if !should_be_active && let Some(sid) = session_id.get_untracked() {
                        engine_for_session.select_session(None);
                        store.forget(&sid);
                        leptos::task::spawn_local(async move {
                            let _ = delete_copilot_session(sid).await;
                        });
                    }
                });

                // Cleanup session on component unmount.
                on_cleanup(move || {
                    if let Some(sid) = session_id.try_get_untracked().flatten() {
                        store.forget(&sid);
                        leptos::task::spawn_local(async move {
                            let _ = delete_copilot_session(sid).await;
                        });
                    }
                });
            }
            SessionMode::External {
                session_id: external_sid,
            } => {
                engine.select_session(external_sid.get_untracked());
                let engine_for_session = engine.clone();
                Effect::new(move || {
                    let _ = store.generation.get();
                    engine_for_session.select_session(external_sid.get());
                });
            }
        }

        engine
    }

    /// Construct the view handles without starting session effects. Keeping
    /// this separate also lets native tests exercise the real ownership path.
    fn create_view(config: ChatEngineConfig) -> (Self, SessionMode) {
        let store = expect_context::<ChatRunStore>();
        let view_id = store.register_view(config.custom_ws_events, config.on_custom_ws_event);
        let run = RwSignal::new(ChatRun::new(config.context_type.clone()));
        let messages_read = Memo::new(move |_| {
            run.try_get()
                .and_then(|run| run.messages.try_get())
                .unwrap_or_default()
        }).into();
        let chat_state = ChatStateMachine::from_source(Signal::derive(move || {
            run.try_get()
                .map(|run| run.chat_state)
                .unwrap_or_else(super::chat_state::ChatStateData::new)
        }));
        let thinking = ThinkingManager::from_source(Signal::derive(move || {
            run.try_get()
                .map(|run| run.thinking)
                .unwrap_or_else(super::thinking::ThinkingData::new)
        }));
        let session_id = RwSignal::new(None::<String>);
        let session_id_read = session_id.read_only();
        let has_sent_first = RwSignal::new(false);
        let context_type = StoredValue::new(config.context_type);
        let context_content = config.context_content;
        let context_label = StoredValue::new(config.context_label);
        let document_id = config.document_id;
        let view_context = config.view_context;
        let historical_preview = config.historical_preview;
        let before_send = config.before_send;

        let engine = Self {
            run,
            store,
            view_id,
            generation: RwSignal::new(store.generation.get_untracked()),
            messages_read,
            chat_state,
            thinking,
            session_id,
            session_id_read,
            has_sent_first,
            context_type,
            context_content,
            context_label,
            document_id,
            view_context,
            historical_preview,
            before_send,
        };
        on_cleanup(move || store.release_view(view_id));

        (engine, config.session_mode)
    }

    /// Attach this view to a retained session. New-chat sends call this
    /// synchronously before dispatch, so the first frame always has a home.
    pub fn select_session(&self, sid: Option<String>) {
        let previous = self.session_id.get_untracked();
        let generation = self.store.generation.get_untracked();
        let same_workspace = self.generation.get_untracked() == generation;
        if previous == sid && same_workspace {
            return;
        }
        let draft = if same_workspace && previous.is_none() && sid.is_some() {
            self.run.get_untracked()
        } else {
            ChatRun::new(self.context_type.get_value())
        };
        let selected = self.store.attach(self.view_id, sid.as_deref(), draft);
        self.run.set(selected);
        self.session_id.set(sid);
        self.generation.set(generation);
        if previous.is_some() {
            self.has_sent_first.set(false);
        }
    }

    /// Return a rejected new-chat send to its draft without losing the prompt
    /// or error. This is explicit: normal navigation to a blank draft must not
    /// carry messages from an unrelated failed session.
    pub(crate) fn return_failed_draft(&self, failed_session_id: &str) -> bool {
        if self.session_id.try_get_untracked().flatten().as_deref() != Some(failed_session_id)
            || self.generation.try_get_untracked() != self.store.generation.try_get_untracked()
        {
            return false;
        }
        let Some(run) = self.run.try_get_untracked() else {
            return false;
        };
        if run.chat_state.state().get_untracked() != ChatState::Error {
            return false;
        }
        let selected = self.store.attach(self.view_id, None, run);
        self.run.set(selected);
        self.session_id.set(None);
        true
    }

    /// Capture retained state before an async send; it must never follow a
    /// subsequent navigation to a different session.
    pub(crate) fn run(&self) -> ChatRun {
        self.run.get_untracked()
    }

    pub(crate) fn sweep(&self) {
        self.store.sweep();
    }

    // ── Read signals ───────────────────────────────────────────────────

    /// Read signal for messages.
    ///
    /// Returns the handle captured once in [`ChatEngine::new`] — see that
    /// site's comment (KYO-781) for why this must not call `.read_only()`
    /// here: doing so on every access re-panics on a disposed owner instead
    /// of letting the caller's `try_get_untracked()` guard handle it.
    pub fn messages(&self) -> Signal<Vec<ChatMessageItem>> {
        self.messages_read
    }

    /// Access the thinking manager.
    pub fn thinking(&self) -> &ThinkingManager {
        &self.thinking
    }

    /// Access the chat state machine.
    pub fn chat_state(&self) -> &ChatStateMachine {
        &self.chat_state
    }

    /// Read signal for session ID.
    ///
    /// Same disposal-safety rationale as [`ChatEngine::messages`] (KYO-781):
    /// returns the handle captured once in [`ChatEngine::new`] instead of
    /// re-deriving a `ReadSignal` (and re-risking a panic) on every call.
    pub fn session_id(&self) -> ReadSignal<Option<String>> {
        self.session_id_read
    }

    // ── Send / add message ─────────────────────────────────────────────

    /// Full send for Ephemeral mode (copilot). Creates optimistic user message,
    /// builds context prefix, transitions state, and calls the copilot server function.
    ///
    /// For External mode callers who handle their own server call, use
    /// `add_user_message()` instead and manage the server call yourself.
    pub fn send(&self, message: String) {
        let sid = match self.session_id.get_untracked() {
            Some(sid) => sid,
            None => return,
        };

        // Add optimistic user message.
        let _user_msg_id = self.add_user_message(&message);

        // Build context prefix.
        let context_prefix = self.build_context_prefix();

        let ctx_type = self.context_type.try_get_value().flatten();
        self.chat_state.start_sending(&sid);

        let chat_state_err = self.chat_state.snapshot();
        let store = self.store;

        // Compute timezone and time context before entering the async closure.
        let timezone = Some(crate::utils::time::get_user_timezone());
        let time_context = crate::utils::time::get_time_context();
        let time_ctx = if time_context.is_empty() {
            None
        } else {
            Some(time_context)
        };

        let document_id = self.document_id.clone();
        let view_context = self.view_context.map(|context| context.get_untracked()).filter(|value| !value.is_empty());
        let historical_preview = self.historical_preview.is_some_and(|preview| preview.get_untracked());
        let before_send = self.before_send.clone();

        // For ephemeral mode, send via copilot server function.
        // The context_type for the server call comes from config.context_type,
        // which matches the session creation context_type.
        let ctx_type_for_send = ctx_type.unwrap_or_default();
        leptos::task::spawn_local(async move {
            // KYO-536: commit any pending buffer before the copilot ever
            // touches the document, so it always edits the saved content —
            // never a race against an unsaved local edit. A hook that
            // reports failure aborts the send entirely rather than
            // dispatching a copilot message against a document that may
            // not reflect what the user is looking at.
            if let Some(hook) = before_send
                && !(hook)().await
            {
                if chat_state_err.state().try_get_untracked().is_some() {
                    chat_state_err.set_error("The document is not ready to send. Resolve the refresh or save error and try again.");
                }
                return;
            }

            if let Err(e) = send_copilot_message(CopilotMessageRequest {
                session_id: sid,
                message,
                context_type: ctx_type_for_send,
                content: context_prefix,
                timezone,
                current_time_user_tz: time_ctx,
                document_id,
                historical_preview,
                view_context,
            })
            .await
            {
                // Guard: the component may have been disposed while the async
                // call was in flight (user navigated away mid-send).
                if chat_state_err.state().try_get_untracked().is_some() {
                    chat_state_err.set_error(&format!("Failed to send: {e}"));
                    store.sweep();
                }
            }
        });
    }

    /// Add a user message optimistically. Returns the generated message_id.
    ///
    /// Used by External mode callers who handle their own server call.
    /// Also used internally by `send()` for Ephemeral mode.
    pub fn add_user_message(&self, content: &str) -> String {
        // A view can remount while its previous send is still awaiting HTTP.
        // Its optimistic identity must never collide with that earlier turn.
        let user_msg_id = generate_optimistic_message_id();

        self.run.get_untracked().messages.update(|msgs| {
            msgs.push(ChatMessageItem {
                message_id: user_msg_id.clone(),
                message_type: "user".to_string(),
                content: content.to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                pinned: false,
                status: "complete".to_string(),
                sent_by: None,
                thinking_events: Vec::new(),
                token_usage: None,
            });
        });

        user_msg_id
    }

    /// Build the context prefix for the current message.
    ///
    /// First message: `[{label}]\n{content}`
    /// Subsequent: `[{label} has been updated]\n{content}`
    /// Returns `None` if no context content or it's empty.
    pub fn build_context_prefix(&self) -> Option<String> {
        let content_signal = self.context_content?;
        let content = content_signal.get_untracked();
        if content.is_empty() {
            return None;
        }

        let label = self
            .context_label
            .try_get_value()
            .flatten()
            .unwrap_or_default();
        let is_first = !self.has_sent_first.get_untracked();
        self.has_sent_first.set(true);

        if is_first {
            Some(format!("[{label}]\n{content}"))
        } else {
            Some(format!("[{label} has been updated]\n{content}"))
        }
    }

    /// Request cancellation. Sends cancel_request via WebSocket if state allows.
    pub fn cancel(&self) {
        if !self.chat_state.request_cancel() {
            return;
        }

        self.send_cancel_ws();
    }

    #[cfg(target_arch = "wasm32")]
    fn send_cancel_ws(&self) {
        let ws_ctx = use_context::<WebSocketContext>();

        let ws_connected = ws_ctx.as_ref().is_some_and(|ctx| {
            ctx.connection_state.get_untracked()
                == super::websocket_client::ConnectionState::Connected
        });

        if !ws_connected {
            return;
        }

        let session_id = self.session_id.get_untracked().unwrap_or_default();

        let message_id = self.chat_state.active_message_id().get_untracked()
            .or_else(|| self.chat_state.snapshot().expected_assistant_id().get_untracked());

        let mut payload = serde_json::json!({
            "type": "cancel_request",
            "session_id": session_id,
        });

        // Include message_id when available (Streaming state); during Sending
        // it is not yet set. The frontend subscriber uses it to confirm the
        // right message was cancelled.
        if let Some(mid) = message_id {
            payload["message_id"] = serde_json::Value::String(mid);
        }

        if let Some(ws) = ws_ctx.as_ref() {
            ws.send(payload);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn send_cancel_ws(&self) {}

    /// Set up smart scroll on a container element.
    ///
    /// Scrolls to bottom only when within 100px of bottom, with 50ms debounce
    /// and smooth scroll. Ported from chat_page.rs lines 520-568.
    pub fn setup_scroll(&self, container_ref: NodeRef<leptos::html::Div>) {
        let messages = self.messages_read;

        #[cfg(target_arch = "wasm32")]
        {
            Effect::new(move |_| {
                // Track messages to trigger on change.
                let _ = messages.try_get();

                let container_guard = container_ref.try_read_untracked();
                let Some(container_guard) = container_guard else {
                    return;
                };
                let Some(container) = container_guard.as_ref() else {
                    return;
                };

                let scroll_top = container.scroll_top();
                let scroll_height = container.scroll_height();
                let client_height = container.client_height();
                let distance_from_bottom = scroll_height - scroll_top - client_height;

                // Only auto-scroll if within 100px of bottom.
                // Uses smooth scroll with 50ms debounce, matching chat_page.rs.
                // Fire-and-forget is acceptable — the timeout fires once after 50ms.
                if distance_from_bottom < 100 {
                    let container = container.clone();
                    let timeout = gloo_timers::callback::Timeout::new(50, move || {
                        let opts = web_sys::ScrollIntoViewOptions::new();
                        opts.set_behavior(web_sys::ScrollBehavior::Smooth);
                        // Scroll the last child element into view smoothly.
                        if let Some(last_child) = container.last_element_child() {
                            last_child.scroll_into_view_with_scroll_into_view_options(&opts);
                        }
                    });
                    std::mem::forget(send_wrapper::SendWrapper::new(timeout));
                }
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (container_ref, messages);
        }
    }

    /// Reset all state (for session switches in External mode).
    pub fn reset(&self) {
        self.run.get_untracked().messages.set(Vec::new());
        self.chat_state.reset();
        self.thinking.clear_all();
        self.has_sent_first.set(false);
    }

    /// Set messages directly (for loading history in External mode).
    pub fn set_messages(&self, msgs: Vec<ChatMessageItem>) {
        self.run.get_untracked().messages.set(msgs);
    }

    /// Set messages from a deferred context (async block inside `spawn_local`,
    /// WebSocket callback, timer, etc.) where the component may have been
    /// disposed before this fires. Silently no-ops if the signal is disposed.
    pub fn try_set_messages(&self, msgs: Vec<ChatMessageItem>) {
        if let Some(run) = self.run.try_get_untracked() {
            run.messages.set(msgs);
        }
    }
}

/// Reconcile exactly one send's optimistic row with its persisted identity.
/// History can already contain the durable row when HTTP finally returns.
pub(crate) fn reconcile_user_message_id(
    messages: &mut Vec<ChatMessageItem>,
    optimistic_id: &str,
    persisted_id: &str,
) {
    if persisted_id.is_empty() || optimistic_id == persisted_id {
        return;
    }
    if messages
        .iter()
        .any(|message| message.message_id == persisted_id)
    {
        messages.retain(|message| message.message_id != optimistic_id);
    } else if let Some(message) = messages
        .iter_mut()
        .find(|message| message.message_id == optimistic_id)
    {
        message.message_id = persisted_id.to_string();
    }
}

fn generate_optimistic_message_id() -> String {
    #[cfg(target_arch = "wasm32")]
    let identity = {
        let mut bytes = [0u8; 16];
        let random_filled = leptos::prelude::window()
            .crypto()
            .ok()
            .is_some_and(|crypto| crypto.get_random_values_with_u8_array(&mut bytes).is_ok());
        if !random_filled {
            for byte in &mut bytes {
                *byte = (js_sys::Math::random() * 256.0) as u8;
            }
        }
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    };
    #[cfg(all(not(target_arch = "wasm32"), feature = "ssr"))]
    let identity = uuid::Uuid::new_v4().to_string();
    #[cfg(all(not(target_arch = "wasm32"), not(feature = "ssr")))]
    let identity = {
        static NEXT_MESSAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!(
            "{nanos}-{}",
            NEXT_MESSAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    };
    format!("user-{identity}")
}

// ─── Event identity checks ─────────────────────────────────────────────────

/// Copilot events must match their retained run's context. Main chat uses the
/// session identity established before dispatch (KYO-494).
pub(crate) fn context_type_matches(filter: Option<&str>, event_context_type: Option<&str>) -> bool {
    match filter {
        Some(expected) => event_context_type == Some(expected),
        None => true,
    }
}

/// Default-deny identity comparison: two missing session IDs are not a match.
pub(crate) fn should_handle(
    current_session_id: Option<&str>,
    msg_session_id: Option<&str>,
) -> bool {
    match (current_session_id, msg_session_id) {
        (Some(current), Some(msg)) => current == msg,
        _ => false,
    }
}

/// Errors carry context at data.context_type, unlike thinking events whose
/// context is nested under data.event.context_type (KYO-501).
pub(crate) fn error_event_context_type(data: Option<&serde_json::Value>) -> Option<&str> {
    data.and_then(|d| d.get("context_type"))
        .and_then(|v| v.as_str())
}

/// Read the server's error key exactly; do not conceal a producer/consumer
/// mismatch by accepting a different key (KYO-550).
fn error_event_message(data: Option<&serde_json::Value>) -> String {
    data.and_then(|d| d.get("error"))
        .and_then(|v| v.as_str())
        .unwrap_or("An error occurred")
        .to_string()
}

/// The server sends the entire final answer under `full_content`, not `content`.
fn completion_content(data: Option<&serde_json::Value>) -> Option<&str> {
    data.and_then(|d| d.get("full_content"))
        .and_then(|v| v.as_str())
}

/// Place a chunk at its absolute byte offset in the final answer. A DB
/// snapshot may already include it, or earlier WS frames may have been missed.
/// A gap must wait for the full-content completion rather than fabricating
/// an answer by joining unrelated spans.
fn apply_stream_chunk(
    msgs: &mut Vec<ChatMessageItem>,
    message_id: &str,
    content: &str,
    content_offset: usize,
    timestamp: &str,
) -> bool {
    if let Some(existing) = msgs
        .iter_mut()
        .find(|m| m.message_id == message_id && m.message_type == "assistant")
    {
        if matches!(existing.status.as_str(), "complete" | "error" | "cancelled" | "interrupted") {
            return false;
        }
        if content_offset > existing.content.len() {
            return true;
        }
        let overlap = (existing.content.len() - content_offset).min(content.len());
        if existing.content.as_bytes()[content_offset..content_offset + overlap]
            != content.as_bytes()[..overlap]
            || !content.is_char_boundary(overlap)
        {
            return false;
        }
        existing.content.push_str(&content[overlap..]);
    } else {
        msgs.push(ChatMessageItem {
            message_id: message_id.to_string(),
            message_type: "assistant".to_string(),
            content: if content_offset == 0 {
                content.to_string()
            } else {
                String::new()
            },
            timestamp: timestamp.to_string(),
            pinned: false,
            status: "in_progress".to_string(),
            sent_by: None,
            thinking_events: Vec::new(),
            token_usage: None,
        });
    }
    true
}

fn apply_chat_completion(
    msgs: &mut Vec<ChatMessageItem>,
    message_id: &str,
    full_content: Option<&str>,
    timestamp: &str,
) {
    if let Some(m) = msgs
        .iter_mut()
        .find(|m| m.message_id == message_id && m.message_type == "assistant")
    {
        if let Some(content) = full_content {
            m.content = content.to_string();
        }
        m.status = "complete".to_string();
    } else if let Some(content) = full_content {
        msgs.push(ChatMessageItem {
            message_id: message_id.to_string(),
            message_type: "assistant".to_string(),
            content: content.to_string(),
            timestamp: timestamp.to_string(),
            pinned: false,
            status: "complete".to_string(),
            sent_by: None,
            thinking_events: Vec::new(),
            token_usage: None,
        });
    }
}

fn completion_matches_active_turn(
    state: ChatState,
    active_message_id: Option<&str>,
    active_session_id: Option<&str>,
    event_message_id: &str,
    event_session_id: Option<&str>,
    // None: copilot has no assistant ID in its HTTP response;
    // Some(None): main chat is still awaiting the HTTP-assigned ID.
    required_assistant_id: Option<Option<&str>>,
    messages: &[ChatMessageItem],
) -> bool {
    match state {
        ChatState::Streaming => active_message_id == Some(event_message_id),
        ChatState::Sending => {
            event_session_id.is_some()
            && active_session_id == event_session_id
            // For the main chat, the session alone cannot identify a turn:
            // defer zero-chunk completion until the HTTP send supplies its ID.
            && required_assistant_id.is_none_or(|expected| expected == Some(event_message_id))
            // A delayed duplicate for a completed turn (or an assistant
            // preceding the latest user message) cannot finish a new send.
            && messages.iter().position(|m|
                m.message_type == "assistant" && m.message_id == event_message_id
            ).is_none_or(|idx| idx == messages.len() - 1 && messages[idx].status != "complete")
        }
        _ => false,
    }
}

// ─── Session event reducer ─────────────────────────────────────────────────

/// Apply one session-routed event to retained state. This is shared by the
/// browser subscription and native lifecycle regression tests.
pub(crate) fn handle_run_event(
    run: &super::chat_run_store::ChatRun,
    event: &str,
    msg: &super::websocket_client::WebSocketMessage,
) {
    use super::{ThinkingEvent, TokenUsage};
    let messages = &run.messages;
    let chat_state = &run.chat_state;
    let thinking = &run.thinking;
    match event {
        "agent_thinking" => {
            let data = match &msg.data {
                Some(d) => d,
                None => return,
            };

            let thinking_event: ThinkingEvent = match data
                .get("event")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
            {
                Some(e) => e,
                None => return,
            };

            let token_usage: Option<TokenUsage> = data
                .get("token_usage")
                .and_then(|v| serde_json::from_value(v.clone()).ok());

            let msg_message_id = match &msg.message_id {
                Some(m) => m.clone(),
                None => return,
            };

            // Delayed reasoning can enrich a terminal reply, but must never
            // restart its run or keep it retained after its view is released.
            let terminal_status = messages.with_untracked(|messages| {
                messages.iter().find(|message| message.message_id == msg_message_id)
                    .filter(|message| matches!(message.status.as_str(), "complete" | "error" | "cancelled" | "interrupted"))
                    .map(|message| message.status.clone())
            });
            if let Some(status) = terminal_status {
                thinking.handle_thinking_event(&msg_message_id, thinking_event, token_usage);
                if status == "cancelled" {
                    thinking.cancel_thinking(&msg_message_id);
                } else {
                    thinking.complete_thinking(&msg_message_id);
                }
                return;
            }

            // Create an assistant placeholder even when no view is mounted.
            messages.try_update(|msgs| {
                if !msgs.iter().any(|m| m.message_id == msg_message_id) {
                    msgs.push(ChatMessageItem {
                        message_id: msg_message_id.clone(),
                        message_type: "assistant".to_string(),
                        content: String::new(),
                        timestamp: msg.timestamp.clone(),
                        pinned: false,
                        status: "in_progress".to_string(),
                        sent_by: None,
                        thinking_events: Vec::new(),
                        token_usage: None,
                    });
                }
            });

            if chat_state.state().get_untracked() == ChatState::Idle
                && let Some(sid) = msg.session_id.as_deref()
            {
                chat_state.start_sending(sid);
                chat_state.start_streaming(&msg_message_id);
            }

            // Transition to streaming state if still in Sending.
            if let Some(state) = chat_state.state().try_get_untracked()
                && state == ChatState::Sending
                && (run.context_type.is_some()
                    || chat_state
                        .expected_assistant_id()
                        .try_get_untracked()
                        .flatten()
                        .as_deref()
                        == Some(msg_message_id.as_str()))
            {
                chat_state.start_streaming(&msg_message_id);
            }

            // Process thinking event via ThinkingManager.
            thinking.handle_thinking_event(&msg_message_id, thinking_event, token_usage);
        }
        "chat_stream" => {
            let content = msg
                .data
                .as_ref()
                .and_then(|d| d.get("content"))
                .and_then(|v| v.as_str());

            let content = match content {
                Some(c) if !c.is_empty() => c.to_string(),
                _ => return,
            };
            let Some(content_offset) = msg
                .data
                .as_ref()
                .and_then(|d| d.get("content_offset"))
                .and_then(|v| v.as_u64())
                .and_then(|offset| usize::try_from(offset).ok())
            else {
                return;
            };

            let msg_message_id = match &msg.message_id {
                Some(m) => m.clone(),
                None => return,
            };

            let applied = messages
                .try_update(|msgs| {
                    apply_stream_chunk(
                        msgs,
                        &msg_message_id,
                        &content,
                        content_offset,
                        &msg.timestamp,
                    )
                })
                .unwrap_or(false);
            if !applied {
                return;
            }
            if let Some(stream_state) = chat_state.state().try_get_untracked() {
                if stream_state == ChatState::Idle {
                    if let Some(sid) = msg.session_id.as_deref() {
                        chat_state.start_sending(sid);
                    }
                    chat_state.start_streaming(&msg_message_id);
                } else if stream_state == ChatState::Sending
                    && (run.context_type.is_some()
                        || chat_state
                            .expected_assistant_id()
                            .try_get_untracked()
                            .flatten()
                            .as_deref()
                            == Some(msg_message_id.as_str()))
                {
                    chat_state.start_streaming(&msg_message_id);
                }
            }
        }
        "chat_complete" => {
            let msg_message_id = match &msg.message_id {
                Some(m) => m.clone(),
                None => return,
            };

            let state = match chat_state.state().try_get_untracked() {
                Some(s) => s,
                None => return,
            };

            // Cancellation guard: skip if we're in Cancelling or Cancelled state.
            if state == ChatState::Cancelling || state == ChatState::Cancelled {
                return;
            }

            let expected = chat_state
                .expected_assistant_id()
                .try_get_untracked()
                .flatten();
            let required_id = run.context_type.is_none().then_some(expected.as_deref());
            let should_complete = completion_matches_active_turn(
                state,
                chat_state
                    .active_message_id()
                    .try_get_untracked()
                    .flatten()
                    .as_deref(),
                chat_state
                    .active_session_id()
                    .try_get_untracked()
                    .flatten()
                    .as_deref(),
                &msg_message_id,
                msg.session_id.as_deref(),
                required_id,
                &messages.get_untracked(),
            );
            let full_content = completion_content(msg.data.as_ref());
            messages.try_update(|msgs| {
                apply_chat_completion(msgs, &msg_message_id, full_content, &msg.timestamp);
            });

            // Complete thinking via ThinkingManager.
            thinking.complete_thinking(&msg_message_id);

            // Only transition state machine if we're actually in Sending or Streaming.
            if should_complete {
                chat_state.complete();
            }
        }
        "token_usage_update" => {
            let data = match &msg.data {
                Some(d) => d,
                None => return,
            };

            let token_update: TokenUsage = match data
                .get("token_usage")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
            {
                Some(t) => t,
                None => return,
            };

            let msg_message_id = match &msg.message_id {
                Some(m) => m.clone(),
                None => return,
            };

            thinking.update_token_usage(&msg_message_id, token_update);
        }
        "error" => {
            let error_msg = error_event_message(msg.data.as_ref());

            chat_state.set_error(&error_msg);
            messages.update(|messages| {
                if let Some(last) = messages
                    .last_mut()
                    .filter(|message| message.status == "in_progress")
                {
                    last.status = "error".into();
                }
            });
        }
        "request_cancelled" => {
            let msg_message_id = match &msg.message_id {
                Some(m) => m.clone(),
                None => return,
            };

            // Confirm cancellation if this event belongs to the active message OR,
            // when cancelling during Sending (no message_id set yet), if the event
            // session matches the active session. Default-deny (KYO-494): two
            // missing session ids are not a match, so this used `should_handle`
            // instead of `==` — `None == None` would otherwise have been `true`.
            let current_sid = chat_state.active_session_id().get_untracked();
            let is_ours = chat_state.is_active_message(&msg_message_id)
                || (chat_state
                    .active_message_id()
                    .try_get_untracked()
                    .flatten()
                    .is_none()
                    && should_handle(current_sid.as_deref(), msg.session_id.as_deref()));

            if is_ours {
                chat_state.confirm_cancelled();
            }

            // Update the assistant message to show it was cancelled.
            messages.try_update(|msgs| {
                for m in msgs.iter_mut() {
                    if m.message_id == msg_message_id && m.message_type == "assistant" {
                        m.content = "_Request cancelled by user._".to_string();
                        m.status = "cancelled".to_string();
                    }
                }
            });

            // Cancel thinking via ThinkingManager.
            thinking.cancel_thinking(&msg_message_id);
        }
        "shared_chat_message" => {
            let Some(data) = &msg.data else { return };
            // Extract message fields from data
            let message_id = data
                .get("message_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let client_msg_id = data
                .get("client_msg_id")
                .and_then(|v| v.as_str())
                .map(String::from);
            let content = data
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let msg_type = data
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let timestamp = data
                .get("timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sent_by: Option<crate::server_fns::chat::SessionUser> = data
                .get("sent_by")
                .and_then(|v| serde_json::from_value(v.clone()).ok());

            let mut msgs = messages.get_untracked();

            // The durable row may already have arrived through history.
            // Reconcile before deduplication so two rows cannot share its key.
            if let Some(ref cid) = client_msg_id {
                reconcile_user_message_id(&mut msgs, cid, &message_id);
            }
            let deduped = msgs.iter().any(|message| message.message_id == message_id);

            if !deduped {
                // Add new message from other user
                msgs.push(ChatMessageItem {
                    message_id,
                    message_type: msg_type,
                    content,
                    timestamp,
                    pinned: false,
                    status: "complete".to_string(),
                    sent_by,
                    thinking_events: Vec::new(),
                    token_usage: None,
                });
            }

            messages.set(msgs);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    //! KYO-494: chat_stream/chat_complete/agent_thinking events leaked
    //! across sessions because `should_handle` treated "the engine has no
    //! session id yet" as "admit everything." These tests cover the
    //! default-deny invariant directly on the pure filtering functions —
    //! no WASM/reactive-owner harness needed, since `should_handle` and
    //! `context_type_matches` take plain values, not signals.

    use super::*;

    #[test]
    fn completion_before_any_chunk_creates_terminal_message() {
        let data = serde_json::json!({ "full_content": "entire answer" });
        let mut msgs = Vec::new();
        apply_chat_completion(&mut msgs, "a1", completion_content(Some(&data)), "now");
        assert_eq!(msgs[0].content, "entire answer");
        assert_eq!(msgs[0].status, "complete");
        assert!(!apply_stream_chunk(&mut msgs, "a1", "entire", 0, "now"));
        assert_eq!(msgs[0].content, "entire answer");
        let empty = serde_json::json!({ "full_content": "" });
        apply_chat_completion(&mut msgs, "a2", completion_content(Some(&empty)), "now");
        assert_eq!(msgs[1].content, "");
        assert_eq!(msgs[1].status, "complete");
    }

    #[test]
    fn completion_replaces_partial_stream_content() {
        let mut msgs = Vec::new();
        assert!(apply_stream_chunk(&mut msgs, "a1", "part", 0, "now"));
        let data = serde_json::json!({ "full_content": "partial answer" });
        apply_chat_completion(&mut msgs, "a1", completion_content(Some(&data)), "now");
        assert_eq!(msgs[0].content, "partial answer");
        assert_eq!(msgs[0].status, "complete");
    }

    #[test]
    fn stream_offset_places_repeated_chunks_after_db_prefix() {
        let mut msgs = Vec::new();
        apply_chat_completion(&mut msgs, "a1", Some("abc"), "now");
        msgs[0].status = "in_progress".to_string();
        // The first received frame repeats the DB prefix; its absolute
        // offset proves it is new text, not a replay of earlier bytes.
        assert!(apply_stream_chunk(&mut msgs, "a1", "abc", 3, "now"));
        assert_eq!(msgs[0].content, "abcabc");
        assert!(apply_stream_chunk(&mut msgs, "a1", "abc", 6, "now"));
        assert_eq!(msgs[0].content, "abcabcabc");
        // An older frame arriving after a newer DB snapshot cannot rewind it.
        assert!(apply_stream_chunk(&mut msgs, "a1", "abc", 3, "now"));
        assert_eq!(msgs[0].content, "abcabcabc");

        let mut fresh = Vec::new();
        apply_stream_chunk(&mut fresh, "a2", "ha", 0, "now");
        apply_stream_chunk(&mut fresh, "a2", "ha", 2, "now");
        assert_eq!(fresh[0].content, "haha");
    }

    #[test]
    fn stream_gap_waits_for_full_completion_without_corrupting_answer() {
        let mut msgs = Vec::new();
        apply_chat_completion(&mut msgs, "a1", Some("abc"), "now");
        msgs[0].status = "in_progress".to_string();
        // A returning client missed the bytes at offsets 3..6. The later
        // frame is retained only as progress state; it cannot be appended.
        assert!(apply_stream_chunk(&mut msgs, "a1", "ghi", 6, "now"));
        assert_eq!(msgs[0].content, "abc");
        apply_chat_completion(&mut msgs, "a1", Some("abcdefghi"), "now");
        assert_eq!(msgs[0].content, "abcdefghi");
        assert_eq!(msgs[0].status, "complete");
    }

    #[test]
    fn stream_offset_handles_overlap_and_utf8_bytes() {
        let mut msgs = Vec::new();
        apply_chat_completion(&mut msgs, "a1", Some("éab"), "now");
        msgs[0].status = "in_progress".to_string();
        assert!(apply_stream_chunk(&mut msgs, "a1", "abc", 2, "now"));
        assert_eq!(msgs[0].content, "éabc");
    }

    #[test]
    fn sending_requires_exact_assistant_id_even_when_the_row_is_absent() {
        let mut msgs = Vec::new();
        let accepts = |expected: Option<&str>, id: &str, rows: &[ChatMessageItem]| {
            completion_matches_active_turn(
                ChatState::Sending, None, Some("s1"), id, Some("s1"), Some(expected), rows,
            )
        };
        // A may be another members turn. Before HTTP resolves, neither A
        // nor the actual zero-chunk B can finish our send.
        assert!(!accepts(None, "a", &msgs));
        assert!(!accepts(None, "b", &msgs));
        apply_chat_completion(&mut msgs, "b", Some(""), "now");
        assert!(!accepts(None, "b", &msgs));
        // The page reconciles this terminal B row when HTTP identifies B.
        assert!(!accepts(Some("b"), "a", &msgs));
        assert!(accepts(Some("b"), "b", &[]));
        assert!(!completion_matches_active_turn(
            ChatState::Sending, None, Some("s1"), "b", Some("s2"), Some(Some("b")), &[],
        ));
        assert!(completion_matches_active_turn(
            ChatState::Streaming, Some("b"), Some("s1"), "b", Some("s1"), Some(Some("b")), &[],
        ));
        assert!(!completion_matches_active_turn(
            ChatState::Streaming, Some("a"), Some("s1"), "b", Some("s1"), Some(Some("b")), &[],
        ));
    }

    // ── should_handle: default-deny on missing identity ─────────────────

    #[test]
    fn should_handle_denies_when_engine_has_no_session_yet() {
        // The exact reproduction from the ticket: a brand-new chat (no
        // session id yet) must not admit another session's event.
        assert!(!should_handle(None, Some("other-session")));
    }

    #[test]
    fn should_handle_denies_when_event_carries_no_session_id() {
        assert!(!should_handle(Some("sess-1"), None));
    }

    #[test]
    fn should_handle_denies_when_both_sides_are_missing() {
        // Regression guard for the `request_cancelled` handler's old `==`
        // comparison, where `None == None` evaluated to `true`.
        assert!(!should_handle(None, None));
    }

    #[test]
    fn should_handle_denies_mismatched_sessions() {
        assert!(!should_handle(Some("sess-1"), Some("sess-2")));
    }

    #[test]
    fn should_handle_admits_matching_sessions() {
        assert!(should_handle(Some("sess-1"), Some("sess-1")));
    }

    // ── context_type_matches ─────────────────────────────────────────────

    #[test]
    fn context_type_matches_no_filter_admits_anything() {
        // Main chat mode: context_type is None, so this check never gates.
        assert!(context_type_matches(None, None));
        assert!(context_type_matches(None, Some("dashboard_copilot")));
    }

    #[test]
    fn context_type_matches_requires_exact_match_when_filtering() {
        assert!(context_type_matches(
            Some("dashboard_copilot"),
            Some("dashboard_copilot")
        ));
        assert!(!context_type_matches(Some("dashboard_copilot"), Some("chart_copilot")));
        assert!(!context_type_matches(Some("dashboard_copilot"), None));
    }

    // ── Copilot path: its own None-session window must stay closed ──────
    //
    // Ephemeral (copilot) engines set context_type = Some(..) and start
    // with session_id = None until `create_copilot_session` resolves. That
    // window must be denied exactly like the main chat's, not treated as
    // "context matched, so admit it anyway."

    #[test]
    fn copilot_pending_session_denies_events_even_with_matching_context_type() {
        let ctx_filter = Some("dashboard_copilot");
        let event_context_type = Some("dashboard_copilot");
        assert!(
            context_type_matches(ctx_filter, event_context_type),
            "context_type matching is a precondition, not the full story"
        );

        // The engine's own session_id is still None (session creation
        // in flight) — an event for some other copilot session must be
        // denied, not admitted just because the context_type matched.
        let engine_session_id: Option<&str> = None;
        assert!(!should_handle(engine_session_id, Some("some-other-copilot-session")));
    }

    #[test]
    fn copilot_admits_once_its_own_session_is_established() {
        let ctx_filter = Some("dashboard_copilot");
        let event_context_type = Some("dashboard_copilot");
        assert!(context_type_matches(ctx_filter, event_context_type));
        assert!(should_handle(Some("copilot-sess-1"), Some("copilot-sess-1")));
    }

    // ── KYO-501: the `error` handler ──────────────────────────────────────
    //
    // The `error` handler now routes through `should_handle_event`
    // (context_type_matches + should_handle) instead of an inline
    // context_type-only check that never looked at `msg.session_id` at
    // all. `should_handle_event` itself is a wasm32-only closure over
    // reactive signals and can't be called directly from a host-side unit
    // test, so these compose its two pure dependencies the same way it
    // does — and drive `error_event_context_type` with realistic `error`
    // event JSON, so the top-level-vs-nested `context_type` extraction
    // (the detail this ticket calls out explicitly) is exercised too, not
    // just asserted about in prose.

    #[test]
    fn error_handler_denies_cross_session_event_on_main_chat() {
        // Main chat page: context_type = None, so the context gate never
        // fires — before KYO-501 this meant the inline check was skipped
        // entirely and every session's `error` event was admitted. Engine
        // is mounted on its own session, but the event belongs to another
        // one of the user's sessions.
        let ctx_filter: Option<&str> = None;
        let data = serde_json::json!({ "message": "boom" });
        let event_context_type = error_event_context_type(Some(&data));
        assert!(context_type_matches(ctx_filter, event_context_type));

        let engine_session_id = Some("sess-own");
        let event_session_id = Some("sess-other");
        assert!(
            !should_handle(engine_session_id, event_session_id),
            "a cross-session error event must not be admitted just because context_type has no filter"
        );
    }

    #[test]
    fn error_handler_admits_own_session_event() {
        let ctx_filter: Option<&str> = None;
        let data = serde_json::json!({ "message": "boom" });
        let event_context_type = error_event_context_type(Some(&data));
        assert!(context_type_matches(ctx_filter, event_context_type));

        let engine_session_id = Some("sess-own");
        let event_session_id = Some("sess-own");
        assert!(
            should_handle(engine_session_id, event_session_id),
            "an error event for the engine's own session must still be admitted"
        );
    }

    #[test]
    fn error_handler_denies_context_type_mismatch() {
        // Copilot sidebar: session ids match, but the event's context_type
        // belongs to a different copilot surface. Must still be denied.
        let ctx_filter = Some("dashboard_copilot");
        let data = serde_json::json!({ "context_type": "chart_copilot", "message": "boom" });
        let event_context_type = error_event_context_type(Some(&data));
        assert!(!context_type_matches(ctx_filter, event_context_type));
    }

    #[test]
    fn error_event_context_type_reads_top_level_not_nested() {
        // The real risk this ticket calls out: `error` events carry
        // `context_type` at the top level, unlike `agent_thinking`'s
        // (nested at `data.event.context_type`). Reading the wrong path
        // would silently disable the context filter for every copilot
        // `error` event.
        let top_level = serde_json::json!({ "context_type": "dashboard_copilot" });
        assert_eq!(error_event_context_type(Some(&top_level)), Some("dashboard_copilot"));

        // agent_thinking's shape must NOT be read as a match here.
        let nested_like_agent_thinking =
            serde_json::json!({ "event": { "context_type": "dashboard_copilot" } });
        assert_eq!(error_event_context_type(Some(&nested_like_agent_thinking)), None);

        assert_eq!(error_event_context_type(None), None);
    }

    // ── error_event_message (KYO-550) ────────────────────────────────────
    //
    // send_error writes the reason under "error"; the handler used to read
    // "message", which is never present, so every server-sent chat error
    // rendered as the generic fallback. These pin the corrected key and
    // guard against silently re-adding a fallback to "message".

    #[test]
    fn error_event_message_reads_the_real_send_error_payload_shape() {
        // Exact shape `send_error` (kyomi-auth::websocket::helpers) builds:
        // `json!({ "error": error_message })`, optionally with `error_code`
        // and `context_type` alongside it.
        let payload = serde_json::json!({
            "error": "LLM budget exhausted for this workspace",
            "error_code": "budget_exhausted",
            "context_type": "dashboard_copilot",
        });
        assert_eq!(
            error_event_message(Some(&payload)),
            "LLM budget exhausted for this workspace"
        );
    }

    #[test]
    fn error_event_message_falls_back_when_no_reason_is_present() {
        assert_eq!(error_event_message(None), "An error occurred");
        assert_eq!(
            error_event_message(Some(&serde_json::json!({}))),
            "An error occurred"
        );
    }

    #[test]
    fn error_event_message_does_not_fall_back_to_the_message_key() {
        // Regression guard: this is the bug itself. A payload that carries
        // "message" instead of "error" must NOT be read — if it were, this
        // test would be re-creating the exact dual-key fallback KYO-550
        // removed, which would hide a future producer/consumer mismatch
        // instead of surfacing it.
        let payload = serde_json::json!({ "message": "should not be read" });
        assert_eq!(error_event_message(Some(&payload)), "An error occurred");
    }
}

// KYO-781: requires `Owner`/`RwSignal` disposal directly from `reactive_graph`,
// exercised natively (no WASM/browser needed — the panic is a pure reactive-graph
// arena mechanism). Gated on `feature = "ssr"` to match this crate's convention
// for tests that touch the reactive graph rather than pure functions — see
// `chat_page.rs`'s `tests_disposal_scope` module (KYO-548) and
// docs/standards (kyomi-ui tests need `--features ssr`, otherwise this module
// silently does not compile and a zero-test run looks green).
#[cfg(all(test, feature = "ssr"))]
mod tests_disposal_safety {
    //! Page-owned read projections remain safe to query after disposal. The
    //! production create_view path is exercised without browser session effects.

    use super::*;
    use crate::components::chat::{ChatState, ThinkingEvent};

    // ── ChatEngine's own accessor pattern (messages / session_id) ──────────
    //
    fn build_real_engine_for_disposal_test() -> ChatEngine {
        if use_context::<ChatRunStore>().is_none() { provide_context(ChatRunStore::new()); }
        ChatEngine::create_view(ChatEngineConfig {
            session_mode: SessionMode::External { session_id: Signal::derive(|| None) },
            context_type: None, custom_ws_events: vec![], on_custom_ws_event: None,
            context_content: None, context_label: None, document_id: None, view_context: None, historical_preview: None, before_send: None,
        }).0
    }

    #[test]
    fn copilot_message_projection_tracks_frames_after_session_attachment() {
        let owner = Owner::new();
        owner.with(|| {
            let store = ChatRunStore::new();
            provide_context(store);
            let engine = ChatEngine::create_view(ChatEngineConfig {
                session_mode: SessionMode::Ephemeral { context_type: "dashboard_copilot".into(), active: None },
                context_type: Some("dashboard_copilot".into()),
                custom_ws_events: vec![], on_custom_ws_event: None,
                context_content: None, context_label: None, document_id: None, view_context: None, historical_preview: None, before_send: None,
            }).0;
            let messages = engine.messages();
            let rendered = Memo::new(move |_| messages.get());
            assert!(rendered.get().is_empty());
            engine.select_session(Some("copilot-1".into()));
            assert!(rendered.get().is_empty());
            store.receive("chat_stream", serde_json::from_value(serde_json::json!({
                "type": "chat_stream", "session_id": "copilot-1", "message_id": "reply",
                "data": { "content": "visible reply", "content_offset": 0, "context_type": "dashboard_copilot" }
            })).expect("valid copilot frame"));
            assert_eq!(rendered.get()[0].content, "visible reply");
        });
        owner.cleanup();
    }

    #[test]
    fn page_disposal_releases_only_the_view_and_returning_engine_reads_shared_run() {
        let layout = Owner::new();
        let store = layout.with(|| {
            let store = ChatRunStore::new();
            provide_context(store);
            store
        });
        let page = layout.child();
        let engine = page.with(build_real_engine_for_disposal_test);
        engine.select_session(Some("session-a".into()));
        engine.chat_state().start_sending("session-a");
        let pending = engine.run();
        page.cleanup();
        assert!(engine.messages().try_get_untracked().is_none());
        let frame = |text: &str, offset: usize| serde_json::from_value(serde_json::json!({
            "type": "chat_stream", "session_id": "session-a", "message_id": "answer",
            "data": { "content": text, "content_offset": offset }
        })).expect("valid chat stream fixture");
        store.receive("chat_stream", frame("received while away", 0));
        // The HTTP continuation also holds the original run, not a page handle.
        pending.chat_state.expect_assistant("session-a", "missing-token", "answer");
        let returned_page = layout.child();
        let returned = returned_page.with(build_real_engine_for_disposal_test);
        returned.select_session(Some("session-a".into()));
        assert_eq!(returned.messages().get_untracked()[0].content, "received while away");
        store.receive("chat_stream", frame(" and after return", 19));
        assert_eq!(returned.messages().get_untracked()[0].content, "received while away and after return");
        returned.select_session(Some("session-b".into()));
        assert!(returned.messages().get_untracked().is_empty());
        store.receive("chat_stream", frame(" in background", 36));
        assert!(returned.messages().get_untracked().is_empty());
        returned.select_session(Some("session-a".into()));
        assert!(returned.messages().get_untracked()[0].content.ends_with(" in background"));
        returned_page.cleanup();
        layout.cleanup();
    }

    #[test]
    fn remounted_view_retry_keeps_both_optimistic_send_identities_distinct() {
        let layout = Owner::new();
        layout.with(|| provide_context(ChatRunStore::new()));
        let page = layout.child();
        let engine = page.with(build_real_engine_for_disposal_test);
        engine.select_session(Some("session".into()));
        let first_id = engine.add_user_message("same prompt");
        engine.chat_state().start_sending("session");
        let pending = engine.run();
        page.cleanup();
        let returned_page = layout.child();
        let returned = returned_page.with(build_real_engine_for_disposal_test);
        returned.select_session(Some("session".into()));
        // A failed history request leaves the retained optimistic row intact.
        returned.chat_state().request_cancel();
        pending.chat_state.confirm_cancelled();
        let second_id = returned.add_user_message("same prompt");
        returned.chat_state().start_sending("session");
        assert_ne!(first_id, second_id);
        pending
            .messages
            .update(|messages| reconcile_user_message_id(messages, &first_id, "persisted-first"));
        let rows = returned.messages().get_untracked();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].message_id, "persisted-first");
        assert_eq!(rows[1].message_id, second_id);
        returned_page.cleanup();
        layout.cleanup();
    }

    #[test]
    fn persisted_history_before_http_is_idempotent_and_preserves_next_prompt() {
        let owner = Owner::new();
        owner.with(|| {
            let engine = build_real_engine_for_disposal_test();
            let first_id = engine.add_user_message("repeat");
            let second_id = engine.add_user_message("repeat");
            let mut durable = engine.messages().get_untracked()[0].clone();
            durable.message_id = "persisted-first".into();
            durable.pinned = true;
            engine
                .run()
                .messages
                .update(|messages| messages.push(durable));
            for _ in 0..2 {
                engine.run().messages.update(|messages| {
                    reconcile_user_message_id(messages, &first_id, "persisted-first")
                });
            }
            let rows = engine.messages().get_untracked();
            assert_eq!(rows.len(), 2);
            assert!(rows.iter().any(|row| row.message_id == second_id));
            assert!(
                rows.iter()
                    .any(|row| row.message_id == "persisted-first" && row.pinned)
            );
        });
        owner.cleanup();
    }

    #[test]
    fn shared_broadcast_after_history_then_late_http_keeps_one_durable_key() {
        let owner = Owner::new();
        owner.with(|| {
            let engine = build_real_engine_for_disposal_test();
            engine.select_session(Some("shared-session".into()));
            let optimistic_id = engine.add_user_message("same prompt");
            let run = engine.run();
            let mut persisted = engine.messages().get_untracked()[0].clone();
            persisted.message_id = "persisted-user".into();
            persisted.pinned = true;
            persisted.timestamp = "durable-timestamp".into();
            run.messages.update(|messages| messages.insert(0, persisted));
            let broadcast = serde_json::from_value(serde_json::json!({
                "type": "shared_chat_message", "session_id": "shared-session",
                "data": {
                    "message_id": "persisted-user", "client_msg_id": optimistic_id,
                    "content": "same prompt", "type": "user", "timestamp": "broadcast-timestamp"
                }
            })).expect("valid shared-chat broadcast");
            for _ in 0..2 {
                handle_run_event(&run, "shared_chat_message", &broadcast);
                run.messages.update(|messages| {
                    reconcile_user_message_id(messages, &optimistic_id, "persisted-user")
                });
            }
            let messages = engine.messages().get_untracked();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].message_id, "persisted-user");
            assert!(messages[0].pinned);
            assert_eq!(messages[0].timestamp, "durable-timestamp");
        });
        owner.cleanup();
    }

    #[test]
    fn rejected_new_session_preserves_failed_draft_and_retry_but_navigation_is_clean() {
        let owner = Owner::new();
        owner.with(|| {
            let engine = build_real_engine_for_disposal_test();
            let prompt_id = engine.add_user_message("first prompt");
            engine.select_session(Some("rejected".into()));
            let rejected_run = engine.run();
            rejected_run.chat_state.start_sending("rejected");
            let mut error_row = engine.messages().get_untracked()[0].clone();
            error_row.message_id = "error-first-send".into();
            error_row.message_type = "assistant".into();
            error_row.status = "error".into();
            error_row.content = "request rejected".into();
            rejected_run.messages.update(|messages| messages.push(error_row));
            rejected_run.chat_state.set_error("request rejected");
            engine.return_failed_draft("rejected");
            engine.select_session(None); // external pending SID effect
            assert_eq!(engine.messages().get_untracked()[0].message_id, prompt_id);
            assert_eq!(
                engine.chat_state().state().get_untracked(),
                ChatState::Error
            );
            assert_eq!(
                engine.chat_state().error().get_untracked().as_deref(),
                Some("request rejected")
            );
            assert!(!engine.chat_state().can_send.get_untracked());
            // The browser's 100ms error timer makes the input available. SSR
            // does not run timers, so advance via the public state reset.
            // The failure banner and retained prompt were checked above.
            engine.chat_state().reset();
            assert!(engine.chat_state().can_send.get_untracked());
            assert_eq!(engine.messages().get_untracked()[0].message_id, prompt_id);
            assert_eq!(engine.messages().get_untracked()[1].content, "request rejected");
            engine.add_user_message("retry prompt");
            engine.select_session(Some("retry-session".into()));
            engine.chat_state().start_sending("retry-session");
            assert_eq!(engine.messages().get_untracked().len(), 3);
            assert_eq!(engine.chat_state().error().get_untracked(), None);
            engine.select_session(Some("unrelated".into()));
            engine.return_failed_draft("rejected"); // stale HTTP failure
            assert!(engine.messages().get_untracked().is_empty());
            assert_eq!(
                engine.session_id().get_untracked().as_deref(),
                Some("unrelated")
            );
            engine.select_session(None);
            assert!(engine.messages().get_untracked().is_empty());
            assert_eq!(engine.chat_state().state().get_untracked(), ChatState::Idle);
        });
        owner.cleanup();
    }

    #[test]
    fn chat_engine_accessors_read_and_track_correctly_while_owner_is_alive() {
        let owner = Owner::new();
        owner.with(|| {
            let engine = build_real_engine_for_disposal_test();
            assert_eq!(engine.messages().try_get_untracked(), Some(Vec::new()));
            assert_eq!(engine.session_id().try_get_untracked(), Some(None));

            // Prove the read is live through the SAME stored handle, not a
            // decoy that happens to match the initial value — drive the
            // mutation through the engine's own real public API
            // (`add_user_message`) rather than poking the private field
            // directly, so this also exercises the real write path.
            let msg_id = engine.add_user_message("hi");
            engine.session_id.set(Some("sess-1".to_string()));
            assert_eq!(
                engine.messages().try_get_untracked().map(|m| m.len()),
                Some(1)
            );
            assert_eq!(
                engine
                    .messages()
                    .try_get_untracked()
                    .and_then(|m| m.first().map(|m| m.message_id.clone())),
                Some(msg_id)
            );
            assert_eq!(
                engine.session_id().try_get_untracked(),
                Some(Some("sess-1".to_string()))
            );
        });
    }

    #[test]
    fn chat_engine_accessors_do_not_panic_after_owner_disposal() {
        // Exact reproduction shape of the reported panic: construct the
        // engine, dispose the owning scope (simulating navigating away from
        // /chat mid-stream), THEN call the accessor for the first time —
        // exactly as chat_page.rs's send-handler `spawn_local` continuation
        // calls `engine.messages()` for the first time when it resumes
        // post-`.await`, by which point the page may already be disposed.
        // Calling the accessor BEFORE disposal would not distinguish this
        // fix from the original bug: the original `.read_only()` only
        // panics when it re-derives a handle from an already-disposed
        // signal, which requires the call to happen after disposal.
        let owner = Owner::new();
        let engine = owner.with(build_real_engine_for_disposal_test);

        // `Owner::cleanup()` is the same explicit disposal path used in
        // production (route unmount), not reliance on Rust's Drop/refcounting
        // — matches `chat_page.rs`'s `tests_disposal_scope` convention.
        owner.cleanup();

        assert_eq!(
            engine.messages().try_get_untracked(),
            None,
            "ChatEngine::messages() must not panic after owner disposal"
        );
        assert_eq!(
            engine.session_id().try_get_untracked(),
            None,
            "ChatEngine::session_id() must not panic after owner disposal"
        );
    }

    // ── ChatStateMachine — real constructor, real accessors ────────────────

    #[test]
    fn chat_state_machine_accessors_read_and_track_correctly_while_owner_is_alive() {
        let owner = Owner::new();
        owner.with(|| {
            let chat_state = ChatStateMachine::new();
            assert_eq!(
                chat_state.state().try_get_untracked(),
                Some(ChatState::Idle)
            );
            assert_eq!(chat_state.active_message_id().try_get_untracked(), Some(None));
            assert_eq!(chat_state.active_session_id().try_get_untracked(), Some(None));
            assert_eq!(chat_state.error().try_get_untracked(), Some(None));

            // Prove reads are live through the SAME stored handle.
            chat_state.start_sending("sess-1");
            assert_eq!(
                chat_state.state().try_get_untracked(),
                Some(ChatState::Sending)
            );
            assert_eq!(
                chat_state.active_session_id().try_get_untracked(),
                Some(Some("sess-1".to_string()))
            );

            chat_state.start_streaming("msg-1");
            assert_eq!(
                chat_state.state().try_get_untracked(),
                Some(ChatState::Streaming)
            );
            assert_eq!(
                chat_state.active_message_id().try_get_untracked(),
                Some(Some("msg-1".to_string()))
            );
        });
    }

    #[test]
    fn chat_state_machine_accessors_do_not_panic_after_owner_disposal() {
        // This is the real-world trigger from chat_page.rs's send handler:
        // `chat_state_inner.state().try_get_untracked().is_some()` is used as
        // a disposal guard before calling `.reset()`/`.set_error()` inside a
        // `spawn_local` continuation — i.e. `.state()` is called for the
        // FIRST time post-disposal, not obtained beforehand. Calling the
        // accessor before disposal would not distinguish this fix from the
        // original bug: `.read_only()` only panics when it re-derives a
        // handle from an already-disposed signal, so the accessor call must
        // happen after `owner.cleanup()` to reproduce it.
        let owner = Owner::new();
        let chat_state = owner.with(ChatStateMachine::new);

        owner.cleanup();

        assert_eq!(
            chat_state.state().try_get_untracked(),
            None,
            "ChatStateMachine::state() must not panic after owner disposal"
        );
        assert_eq!(
            chat_state.active_message_id().try_get_untracked(),
            None,
            "ChatStateMachine::active_message_id() must not panic after owner disposal"
        );
        assert_eq!(
            chat_state.active_session_id().try_get_untracked(),
            None,
            "ChatStateMachine::active_session_id() must not panic after owner disposal"
        );
        assert_eq!(
            chat_state.error().try_get_untracked(),
            None,
            "ChatStateMachine::error() must not panic after owner disposal"
        );
    }

    // ── ThinkingManager — real constructor, real accessor ───────────────────

    #[test]
    fn thinking_manager_state_reads_and_tracks_correctly_while_owner_is_alive() {
        let owner = Owner::new();
        owner.with(|| {
            let thinking = ThinkingManager::new();
            assert!(
                thinking.state().try_get_untracked().is_some_and(|m| m.is_empty()),
                "thinking state must start alive and empty"
            );

            thinking.handle_thinking_event(
                "msg-1",
                ThinkingEvent {
                    event_id: "1-0".to_string(),
                    event_type: "agent_thought".to_string(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    title: "Thinking".to_string(),
                    description: None,
                    data: None,
                    duration_ms: None,
                    has_full_text: false,
                },
                None,
            );
            assert!(
                thinking
                    .state()
                    .try_get_untracked()
                    .is_some_and(|m| m.contains_key("msg-1")),
                "read must observe the write through the same stored handle"
            );
        });
    }

    #[test]
    fn thinking_manager_state_does_not_panic_after_owner_disposal() {
        // As with `ChatStateMachine` above: `.state()` must be called for
        // the FIRST time after disposal to reproduce the original bug —
        // `.read_only()` only panics when re-deriving from an
        // already-disposed signal.
        let owner = Owner::new();
        let thinking = owner.with(ThinkingManager::new);

        owner.cleanup();

        assert!(
            thinking.state().try_get_untracked().is_none(),
            "ThinkingManager::state() must not panic after owner disposal"
        );
    }
}
