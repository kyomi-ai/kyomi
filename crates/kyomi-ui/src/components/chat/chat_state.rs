// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat State Machine
//!
//! Manages the lifecycle of chat interactions with a clear state machine.
//! Replaces fragmented state (is_loading, is_processing, active_message_id, etc.)
//! with a single source of truth.
//!
//! Ported from `apps/frontend/src/hooks/useChatState.js` — matches React exactly.
//!
//! States:
//! - `Idle`: Ready to send a new message
//! - `Sending`: HTTP request in flight to initiate chat
//! - `Streaming`: Receiving agent response chunks via WebSocket
//! - `Cancelling`: User requested cancellation
//! - `Cancelled`: Cancellation confirmed by backend
//! - `Error`: An error occurred
//!
//! Benefits:
//! - Stop button logic is trivial: `show_stop_button = Sending | Streaming | Cancelling`
//! - Session isolation: Filter WebSocket messages by `active_session_id`
//! - No race conditions: Clear state transitions
//! - Easier debugging: Log all state changes

use leptos::prelude::*;

/// Chat interaction states — matches React's `CHAT_STATES`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatState {
    Idle,
    Sending,
    Streaming,
    Cancelling,
    Cancelled,
    Error,
}

impl ChatState {
    /// Returns the valid states this state can transition to.
    /// Matches React's `VALID_TRANSITIONS` map exactly.
    fn valid_transitions(self) -> &'static [ChatState] {
        match self {
            ChatState::Idle => &[ChatState::Sending],
            ChatState::Sending => &[ChatState::Streaming, ChatState::Cancelling, ChatState::Error, ChatState::Idle],
            ChatState::Streaming => &[ChatState::Idle, ChatState::Cancelling, ChatState::Error],
            ChatState::Cancelling => &[ChatState::Cancelled, ChatState::Idle, ChatState::Error],
            ChatState::Cancelled => &[ChatState::Idle],
            ChatState::Error => &[ChatState::Idle],
        }
    }

    /// Check if transitioning to `target` is valid from this state.
    fn can_transition_to(self, target: ChatState) -> bool {
        self.valid_transitions().contains(&target)
    }
}

impl std::fmt::Display for ChatState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatState::Idle => write!(f, "idle"),
            ChatState::Sending => write!(f, "sending"),
            ChatState::Streaming => write!(f, "streaming"),
            ChatState::Cancelling => write!(f, "cancelling"),
            ChatState::Cancelled => write!(f, "cancelled"),
            ChatState::Error => write!(f, "error"),
        }
    }
}

/// Reactive chat state machine — Leptos equivalent of `useChatState()`.
///
/// Provides read signals for state, active message/session IDs, and error,
/// plus computed signals for UI convenience (can_send, is_streaming, etc.).
///
/// All mutation happens through methods that enforce valid state transitions.
#[derive(Clone)]
pub struct ChatStateMachine {
    /// Current chat state.
    state: RwSignal<ChatState>,
    /// Read-only handle for `state`, captured ONCE in `new()` rather than
    /// derived on every `state()` call — see that method's doc comment
    /// (KYO-781) for why.
    state_read: ReadSignal<ChatState>,
    /// The message ID currently being streamed (set in `start_streaming`).
    active_message_id: RwSignal<Option<String>>,
    /// Read-only handle for `active_message_id`, captured once — see `state_read`.
    active_message_id_read: ReadSignal<Option<String>>,
    /// The session ID for the current chat interaction (set in `start_sending`).
    active_session_id: RwSignal<Option<String>>,
    /// Read-only handle for `active_session_id`, captured once — see `state_read`.
    active_session_id_read: ReadSignal<Option<String>>,
    /// Error message, if any.
    error: RwSignal<Option<String>>,
    /// Read-only handle for `error`, captured once — see `state_read`.
    error_read: ReadSignal<Option<String>>,

    // -- Computed signals (cached, derived from state) --

    /// `true` when `state == Idle` — user can send a new message.
    pub can_send: Signal<bool>,
    /// `true` when `state == Sending`.
    pub is_sending: Signal<bool>,
    /// `true` when `state == Streaming`.
    pub is_streaming: Signal<bool>,
    /// `true` when `state` is `Sending | Streaming | Cancelling` — show stop button.
    pub show_stop_button: Signal<bool>,
    /// `true` when `state` is `Sending` or `Streaming`.
    pub can_cancel: Signal<bool>,
    /// `true` when `state == Cancelling`.
    pub is_cancelling: Signal<bool>,
    /// `true` when `state == Error`.
    pub has_error: Signal<bool>,
}

impl Default for ChatStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatStateMachine {
    /// Create a new chat state machine with all reactive signals.
    pub fn new() -> Self {
        let state = RwSignal::new(ChatState::Idle);
        // KYO-781: capture the read-only handles synchronously here, while
        // this constructor's `Owner` is current — see `ChatEngine::new`'s
        // `messages_read` comment in chat_engine.rs for the full mechanism.
        // `RwSignal::read_only()` allocates a fresh arena node under whatever
        // owner is current AND panics if `self` is already disposed; deferring
        // that call to a later, possibly-post-disposal `state()`/etc. access
        // is exactly what let this state machine panic through a caller's
        // `try_get_untracked()` guard instead of letting it fire.
        let state_read = state.read_only();
        let active_message_id = RwSignal::new(None::<String>);
        let active_message_id_read = active_message_id.read_only();
        let active_session_id = RwSignal::new(None::<String>);
        let active_session_id_read = active_session_id.read_only();
        let error = RwSignal::new(None::<String>);
        let error_read = error.read_only();

        // Computed signals — matches React's computed properties exactly.
        let can_send = Signal::derive(move || state.get() == ChatState::Idle);
        let is_sending = Signal::derive(move || state.get() == ChatState::Sending);
        let is_streaming = Signal::derive(move || state.get() == ChatState::Streaming);
        let show_stop_button = Signal::derive(move || {
            matches!(
                state.get(),
                ChatState::Sending | ChatState::Streaming | ChatState::Cancelling
            )
        });
        let can_cancel = Signal::derive(move || {
            matches!(state.get(), ChatState::Streaming | ChatState::Sending)
        });
        let is_cancelling = Signal::derive(move || state.get() == ChatState::Cancelling);
        let has_error = Signal::derive(move || state.get() == ChatState::Error);

        Self {
            state,
            state_read,
            active_message_id,
            active_message_id_read,
            active_session_id,
            active_session_id_read,
            error,
            error_read,
            can_send,
            is_sending,
            is_streaming,
            show_stop_button,
            can_cancel,
            is_cancelling,
            has_error,
        }
    }

    // -- Read signals --------------------------------------------------------

    /// Read signal for the current chat state.
    ///
    /// Returns the handle captured once in `new()` (KYO-781) rather than
    /// calling `.read_only()` here, which would re-allocate an arena node
    /// under whatever owner is current AND panic if this state machine's
    /// owner is already disposed — defeating callers' `try_get_untracked()`
    /// disposal guards (e.g. `chat_page.rs`'s send handler) before they ever
    /// run.
    pub fn state(&self) -> ReadSignal<ChatState> {
        self.state_read
    }

    /// Read signal for the active message ID. See `state()` for why this
    /// returns the handle captured once in `new()` (KYO-781).
    pub fn active_message_id(&self) -> ReadSignal<Option<String>> {
        self.active_message_id_read
    }

    /// Read signal for the active session ID. See `state()` for why this
    /// returns the handle captured once in `new()` (KYO-781).
    pub fn active_session_id(&self) -> ReadSignal<Option<String>> {
        self.active_session_id_read
    }

    /// Read signal for the error message. See `state()` for why this
    /// returns the handle captured once in `new()` (KYO-781).
    pub fn error(&self) -> ReadSignal<Option<String>> {
        self.error_read
    }

    // -- State transitions ---------------------------------------------------

    /// Internal: transition to a new state with validation.
    /// Matches React's `transition()` — logs invalid transitions but allows them.
    fn transition(&self, new_state: ChatState, reason: &str) {
        let current = self.state.get_untracked();

        if !current.can_transition_to(new_state) {
            tracing::warn!(
                "Invalid chat state transition: {} -> {} (reason: {})",
                current,
                new_state,
                reason
            );
        }

        tracing::debug!(
            "Chat state: {} -> {} (reason: {})",
            current,
            new_state,
            reason
        );

        // Clear error when transitioning away from Error state — matches React.
        if current == ChatState::Error && new_state != ChatState::Error {
            self.error.set(None);
        }

        self.state.set(new_state);
    }

    /// Start sending a new message. Sets `active_session_id`.
    ///
    /// Matches React's `startSending(sessionId)`.
    pub fn start_sending(&self, session_id: &str) {
        self.transition(ChatState::Sending, "start_sending");
        self.active_session_id.set(Some(session_id.to_string()));
    }

    /// Message was sent, now streaming response. Sets `active_message_id`.
    ///
    /// Matches React's `startStreaming(messageId)`.
    pub fn start_streaming(&self, message_id: &str) {
        self.transition(ChatState::Streaming, "start_streaming");
        self.active_message_id.set(Some(message_id.to_string()));
    }

    /// User requested cancellation. Returns `true` if the cancel was accepted.
    ///
    /// Cancellation is accepted from `Streaming` (with or without message_id)
    /// and from `Sending` (session_id is sufficient for the backend to cancel).
    /// Matches React's `requestCancel()`.
    pub fn request_cancel(&self) -> bool {
        let current = self.state.get_untracked();

        if current != ChatState::Streaming && current != ChatState::Sending {
            return false;
        }

        self.transition(ChatState::Cancelling, "request_cancel");
        true
    }

    /// Cancellation confirmed by backend. Auto-resets to Idle after 100ms.
    ///
    /// Matches React's `confirmCancelled()`.
    pub fn confirm_cancelled(&self) {
        self.transition(ChatState::Cancelled, "confirm_cancelled");

        // Auto-reset to Idle after 100ms — matches React's setTimeout.
        self.schedule_auto_reset("auto-reset after cancel");
    }

    /// Response completed successfully. Clears `active_message_id`.
    ///
    /// Matches React's `complete()`.
    pub fn complete(&self) {
        self.transition(ChatState::Idle, "completed");
        self.active_message_id.set(None);
    }

    /// An error occurred. Sets error message and auto-resets to Idle after 100ms.
    ///
    /// Matches React's `setErrorState(errorMessage)`.
    pub fn set_error(&self, msg: &str) {
        self.transition(ChatState::Error, "error");
        self.error.set(Some(msg.to_string()));

        // Auto-reset to Idle after 100ms — matches React's setTimeout.
        self.schedule_auto_reset("auto-reset after error");
    }

    /// Reset to idle (e.g., when switching sessions). Clears all state.
    ///
    /// Matches React's `reset(reason)`.
    pub fn reset(&self) {
        // Only transition if not already idle (avoid idle->idle warnings) — matches React.
        if self.state.get_untracked() != ChatState::Idle {
            self.transition(ChatState::Idle, "manual reset");
        }
        self.active_message_id.set(None);
        self.active_session_id.set(None);
        self.error.set(None);
    }

    // -- Helpers -------------------------------------------------------------

    /// Check if a message ID matches the current active message.
    ///
    /// Matches React's `isActiveMessage(messageId)`.
    pub fn is_active_message(&self, message_id: &str) -> bool {
        self.active_message_id
            .get_untracked()
            .as_deref()
            == Some(message_id)
    }

    /// Check if a session ID matches the current active session.
    ///
    /// Matches React's `isActiveSession(sessionId)`.
    pub fn is_active_session(&self, session_id: &str) -> bool {
        self.active_session_id
            .get_untracked()
            .as_deref()
            == Some(session_id)
    }

    // -- Internal ------------------------------------------------------------

    /// Schedule auto-reset to Idle after 100ms using `gloo_timers::callback::Timeout`.
    ///
    /// Matches React's `setTimeout(() => { transition(IDLE); setActiveMessageId(null); }, 100)`.
    fn schedule_auto_reset(&self, reason: &'static str) {
        // Clone the signals we need to move into the closure.
        let state = self.state;
        let active_message_id = self.active_message_id;

        // On WASM, use gloo-timers. On SSR, auto-reset is a no-op (no timers).
        #[cfg(target_arch = "wasm32")]
        {
            // Store the timeout handle to prevent it from being dropped (which cancels it).
            // SendWrapper is needed because gloo Timeout is !Send but Leptos may require Send.
            use send_wrapper::SendWrapper;
            let timeout = gloo_timers::callback::Timeout::new(100, move || {
                if let Some(current) = state.try_get_untracked() {
                    tracing::debug!("Chat state auto-reset: {current} -> idle (reason: {reason})");
                }
                state.try_set(ChatState::Idle);
                active_message_id.try_set(None);
            });
            // Leak the timeout handle intentionally — it fires once and self-cleans.
            // This matches React's setTimeout which is fire-and-forget.
            std::mem::forget(SendWrapper::new(timeout));
        }

        // Suppress unused variable warnings on SSR.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (state, active_message_id, reason);
        }
    }
}
