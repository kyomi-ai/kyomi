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
            ChatState::Sending => &[
                ChatState::Streaming,
                ChatState::Cancelling,
                ChatState::Error,
                ChatState::Idle,
            ],
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

/// State owned by a retained chat run. Arc signals survive route disposal and
/// are freed when the store and any in-flight operation release the run.
#[derive(Clone)]
pub(crate) struct ChatStateData {
    state: ArcRwSignal<ChatState>,
    active_message_id: ArcRwSignal<Option<String>>,
    expected_assistant_id: ArcRwSignal<Option<String>>,
    sending_token: ArcRwSignal<Option<String>>,
    active_session_id: ArcRwSignal<Option<String>>,
    error: ArcRwSignal<Option<String>>,
}

impl ChatStateData {
    pub(crate) fn new() -> Self {
        Self {
            state: ArcRwSignal::new(ChatState::Idle),
            active_message_id: ArcRwSignal::new(None),
            expected_assistant_id: ArcRwSignal::new(None),
            sending_token: ArcRwSignal::new(None),
            active_session_id: ArcRwSignal::new(None),
            error: ArcRwSignal::new(None),
        }
    }

    pub(crate) fn state(&self) -> ArcReadSignal<ChatState> {
        self.state.read_only()
    }

    pub(crate) fn active_message_id(&self) -> ArcReadSignal<Option<String>> {
        self.active_message_id.read_only()
    }

    pub(crate) fn expected_assistant_id(&self) -> ArcReadSignal<Option<String>> {
        self.expected_assistant_id.read_only()
    }

    pub(crate) fn active_session_id(&self) -> ArcReadSignal<Option<String>> {
        self.active_session_id.read_only()
    }

    pub(crate) fn error(&self) -> ArcReadSignal<Option<String>> {
        self.error.read_only()
    }

    pub fn expect_assistant(&self, session_id: &str, send_token: &str, message_id: &str) -> bool {
        if self.state.get_untracked() != ChatState::Sending
            || self.active_session_id.get_untracked().as_deref() != Some(session_id)
            || self.sending_token.get_untracked().as_deref() != Some(send_token)
        {
            return false;
        }
        self.expected_assistant_id.set(Some(message_id.to_string()));
        true
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
        self.active_message_id.set(None);
        self.expected_assistant_id.set(None);
        self.sending_token.set(None);
    }

    /// Start a page send with a fresh local token that survives only this turn.
    /// Unlike the optimistic user ID, it cannot repeat after an engine reset.
    pub fn start_sending_with_token(&self, session_id: &str, send_token: &str) {
        self.start_sending(session_id);
        self.sending_token.set(Some(send_token.to_string()));
    }

    /// Message was sent, now streaming response. Sets `active_message_id`.
    ///
    /// Matches React's `startStreaming(messageId)`.
    pub fn start_streaming(&self, message_id: &str) {
        self.transition(ChatState::Streaming, "start_streaming");
        self.active_message_id.set(Some(message_id.to_string()));
        self.expected_assistant_id.set(Some(message_id.to_string()));
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
        self.expected_assistant_id.set(None);
        self.sending_token.set(None);
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
        self.expected_assistant_id.set(None);
        self.sending_token.set(None);
        self.active_session_id.set(None);
        self.error.set(None);
    }

    // -- Helpers -------------------------------------------------------------

    /// Check if a message ID matches the current active message.
    ///
    /// Matches React's `isActiveMessage(messageId)`.
    pub fn is_active_message(&self, message_id: &str) -> bool {
        self.active_message_id.get_untracked().as_deref() == Some(message_id)
    }

    /// Check if a session ID matches the current active session.
    ///
    /// Matches React's `isActiveSession(sessionId)`.
    pub fn is_active_session(&self, session_id: &str) -> bool {
        self.active_session_id.get_untracked().as_deref() == Some(session_id)
    }

    // -- Internal ------------------------------------------------------------

    /// Schedule auto-reset to Idle after 100ms using `gloo_timers::callback::Timeout`.
    ///
    /// Matches React's `setTimeout(() => { transition(IDLE); setActiveMessageId(null); }, 100)`.
    fn schedule_auto_reset(&self, reason: &'static str) {
        // Clone the signals we need to move into the closure.
        let state = self.state.clone();
        let active_message_id = self.active_message_id.clone();

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

/// A mounted view of a run's state. Every read handle is created once in the
/// view owner; deferred code should capture `snapshot()` before awaiting.
#[derive(Clone)]
pub struct ChatStateMachine {
    data: Signal<ChatStateData>,
    state_read: Signal<ChatState>,
    active_message_id_read: Signal<Option<String>>,
    expected_assistant_id_read: Signal<Option<String>>,
    active_session_id_read: Signal<Option<String>>,
    error_read: Signal<Option<String>>,
    pub can_send: Signal<bool>,
    pub is_sending: Signal<bool>,
    pub is_streaming: Signal<bool>,
    pub show_stop_button: Signal<bool>,
    pub can_cancel: Signal<bool>,
    pub is_cancelling: Signal<bool>,
    pub has_error: Signal<bool>,
}

impl Default for ChatStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatStateMachine {
    pub fn new() -> Self {
        let data = ChatStateData::new();
        Self::from_source(Signal::derive(move || data.clone()))
    }

    pub(crate) fn from_source(data: Signal<ChatStateData>) -> Self {
        let state_read = Signal::derive(move || {
            data.try_get()
                .and_then(|data| data.state().try_get())
                .unwrap_or(ChatState::Idle)
        });
        let active_message_id_read = Signal::derive(move || {
            data.try_get()
                .and_then(|data| data.active_message_id().try_get())
                .flatten()
        });
        let expected_assistant_id_read = Signal::derive(move || {
            data.try_get()
                .and_then(|data| data.expected_assistant_id().try_get())
                .flatten()
        });
        let active_session_id_read = Signal::derive(move || {
            data.try_get()
                .and_then(|data| data.active_session_id().try_get())
                .flatten()
        });
        let error_read = Signal::derive(move || {
            data.try_get()
                .and_then(|data| data.error().try_get())
                .flatten()
        });
        Self {
            data,
            state_read,
            active_message_id_read,
            expected_assistant_id_read,
            active_session_id_read,
            error_read,
            can_send: Signal::derive(move || {
                let s = state_read.try_get();
                s == Some(ChatState::Idle)
            }),
            is_sending: Signal::derive(move || {
                let s = state_read.try_get();
                s == Some(ChatState::Sending)
            }),
            is_streaming: Signal::derive(move || {
                let s = state_read.try_get();
                s == Some(ChatState::Streaming)
            }),
            show_stop_button: Signal::derive(move || {
                let s = state_read.try_get();
                matches!(
                    s,
                    Some(ChatState::Sending | ChatState::Streaming | ChatState::Cancelling)
                )
            }),
            can_cancel: Signal::derive(move || {
                let s = state_read.try_get();
                matches!(s, Some(ChatState::Sending | ChatState::Streaming))
            }),
            is_cancelling: Signal::derive(move || {
                let s = state_read.try_get();
                s == Some(ChatState::Cancelling)
            }),
            has_error: Signal::derive(move || {
                let s = state_read.try_get();
                s == Some(ChatState::Error)
            }),
        }
    }

    pub(crate) fn snapshot(&self) -> ChatStateData {
        self.data.get_untracked()
    }

    pub fn state(&self) -> Signal<ChatState> {
        self.state_read
    }

    pub fn active_message_id(&self) -> Signal<Option<String>> {
        self.active_message_id_read
    }

    pub fn expected_assistant_id(&self) -> Signal<Option<String>> {
        self.expected_assistant_id_read
    }

    pub fn active_session_id(&self) -> Signal<Option<String>> {
        self.active_session_id_read
    }

    pub fn error(&self) -> Signal<Option<String>> {
        self.error_read
    }

    pub fn start_sending(&self, session_id: &str) {
        self.snapshot().start_sending(session_id)
    }

    pub fn start_sending_with_token(&self, session_id: &str, send_token: &str) {
        self.snapshot()
            .start_sending_with_token(session_id, send_token)
    }

    pub fn start_streaming(&self, message_id: &str) {
        self.snapshot().start_streaming(message_id)
    }

    pub fn expect_assistant(&self, session_id: &str, send_token: &str, message_id: &str) -> bool {
        self.snapshot()
            .expect_assistant(session_id, send_token, message_id)
    }

    pub fn request_cancel(&self) -> bool {
        self.snapshot().request_cancel()
    }

    pub fn confirm_cancelled(&self) {
        self.snapshot().confirm_cancelled()
    }

    pub fn complete(&self) {
        self.snapshot().complete()
    }

    pub fn set_error(&self, msg: &str) {
        self.snapshot().set_error(msg)
    }

    pub fn reset(&self) {
        self.snapshot().reset()
    }

    pub fn is_active_message(&self, message_id: &str) -> bool {
        self.snapshot().is_active_message(message_id)
    }

    pub fn is_active_session(&self, session_id: &str) -> bool {
        self.snapshot().is_active_session(session_id)
    }
}
