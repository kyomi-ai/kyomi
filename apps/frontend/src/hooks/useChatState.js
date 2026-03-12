// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Chat State Machine Hook
 *
 * Manages the lifecycle of chat interactions with a clear state machine.
 * Replaces fragmented state (isLoading, isProcessing, aiMessageIdRef, etc.)
 * with a single source of truth.
 *
 * States:
 * - IDLE: Ready to send a new message
 * - SENDING: HTTP request in flight to initiate chat
 * - STREAMING: Receiving agent response chunks via WebSocket
 * - CANCELLING: User requested cancellation
 * - CANCELLED: Cancellation confirmed by backend
 * - ERROR: An error occurred
 *
 * Benefits:
 * - Stop button logic is trivial: showStopButton = state === 'STREAMING'
 * - Session isolation: Filter WebSocket messages by activeSessionId
 * - No race conditions: Clear state transitions
 * - Easier debugging: Log all state changes
 */

import { useState, useCallback, useRef } from 'react';

// Chat states
export const CHAT_STATES = {
  IDLE: 'idle',
  SENDING: 'sending',
  STREAMING: 'streaming',
  CANCELLING: 'cancelling',
  CANCELLED: 'cancelled',
  ERROR: 'error'
};

// Valid state transitions
const VALID_TRANSITIONS = {
  [CHAT_STATES.IDLE]: [CHAT_STATES.SENDING],
  [CHAT_STATES.SENDING]: [CHAT_STATES.STREAMING, CHAT_STATES.ERROR, CHAT_STATES.IDLE],
  [CHAT_STATES.STREAMING]: [CHAT_STATES.IDLE, CHAT_STATES.CANCELLING, CHAT_STATES.ERROR],
  [CHAT_STATES.CANCELLING]: [CHAT_STATES.CANCELLED, CHAT_STATES.IDLE, CHAT_STATES.ERROR],
  [CHAT_STATES.CANCELLED]: [CHAT_STATES.IDLE],
  [CHAT_STATES.ERROR]: [CHAT_STATES.IDLE]
};

export const useChatState = () => {
  const [state, setState] = useState(CHAT_STATES.IDLE);
  const [activeMessageId, setActiveMessageId] = useState(null);
  const [activeSessionId, setActiveSessionId] = useState(null);
  const [error, setError] = useState(null);

  // For debugging - track state transitions
  const transitionLog = useRef([]);

  // Use refs to track current state to avoid stale closures
  const stateRef = useRef(state);
  const activeMessageIdRef = useRef(activeMessageId);

  stateRef.current = state;
  activeMessageIdRef.current = activeMessageId;

  /**
   * Transition to a new state with validation
   */
  const transition = useCallback((newState, metadata = {}) => {
    const currentState = stateRef.current;
    const validTransitions = VALID_TRANSITIONS[currentState];

    if (!validTransitions.includes(newState)) {
      // Allow transition anyway but log the error
    }

    const timestamp = new Date().toISOString();
    const transitionData = {
      from: currentState,
      to: newState,
      timestamp,
      ...metadata
    };

    transitionLog.current.push(transitionData);

    setState(newState);

    // Clear error when transitioning away from ERROR state
    if (currentState === CHAT_STATES.ERROR && newState !== CHAT_STATES.ERROR) {
      setError(null);
    }
  }, []); // No dependencies - use refs instead

  /**
   * Start sending a new message
   */
  const startSending = useCallback((sessionId) => {
    transition(CHAT_STATES.SENDING, { sessionId });
    setActiveSessionId(sessionId);
  }, [transition]);

  /**
   * Message was sent, now streaming response
   */
  const startStreaming = useCallback((messageId) => {
    transition(CHAT_STATES.STREAMING, { messageId });
    setActiveMessageId(messageId);
  }, [transition]);

  /**
   * User requested cancellation
   */
  const requestCancel = useCallback(() => {
    const currentState = stateRef.current;
    const currentMessageId = activeMessageIdRef.current;

    // Can only cancel when streaming (need message_id to send cancel request to backend)
    if (currentState !== CHAT_STATES.STREAMING) {
      return false;
    }

    if (!currentMessageId) {
      return false;
    }

    transition(CHAT_STATES.CANCELLING, { messageId: currentMessageId });
    return true;
  }, [transition]);

  /**
   * Cancellation confirmed by backend
   */
  const confirmCancelled = useCallback(() => {
    transition(CHAT_STATES.CANCELLED, { messageId: activeMessageIdRef.current });
    // Automatically return to IDLE after a brief delay
    setTimeout(() => {
      transition(CHAT_STATES.IDLE, { reason: 'auto-reset after cancel' });
      setActiveMessageId(null);
    }, 100);
  }, [transition]);

  /**
   * Response completed successfully
   */
  const complete = useCallback(() => {
    transition(CHAT_STATES.IDLE, { messageId: activeMessageIdRef.current, reason: 'completed' });
    setActiveMessageId(null);
  }, [transition]);

  /**
   * An error occurred
   */
  const setErrorState = useCallback((errorMessage) => {
    transition(CHAT_STATES.ERROR, { error: errorMessage, messageId: activeMessageIdRef.current });
    setError(errorMessage);
    // Automatically return to IDLE after a brief delay
    setTimeout(() => {
      transition(CHAT_STATES.IDLE, { reason: 'auto-reset after error' });
      setActiveMessageId(null);
    }, 100);
  }, [transition]);

  /**
   * Reset to idle (e.g., when switching sessions)
   */
  const reset = useCallback((reason = 'manual reset') => {
    // Only transition if not already idle (avoid idle->idle warnings)
    if (stateRef.current !== CHAT_STATES.IDLE) {
      transition(CHAT_STATES.IDLE, { reason });
    }
    setActiveMessageId(null);
    setActiveSessionId(null);
    setError(null);
  }, [transition]);

  /**
   * Check if a message belongs to the current active session
   */
  const isActiveMessage = useCallback((messageId) => {
    return messageId === activeMessageId;
  }, [activeMessageId]);

  /**
   * Check if a session is the active one
   */
  const isActiveSession = useCallback((sessionId) => {
    return sessionId === activeSessionId;
  }, [activeSessionId]);

  // Computed properties for UI
  const canSend = state === CHAT_STATES.IDLE;
  const isSending = state === CHAT_STATES.SENDING;
  const isStreaming = state === CHAT_STATES.STREAMING;
  const showStopButton = state === CHAT_STATES.SENDING || state === CHAT_STATES.STREAMING || state === CHAT_STATES.CANCELLING;
  // Can cancel when streaming AND we have a message_id (from first thinking event)
  const canCancel = state === CHAT_STATES.STREAMING && activeMessageId !== null;
  const isCancelling = state === CHAT_STATES.CANCELLING;
  const hasError = state === CHAT_STATES.ERROR;

  return {
    // Current state
    state,
    activeMessageId,
    activeSessionId,
    error,

    // State transitions
    startSending,
    startStreaming,
    requestCancel,
    confirmCancelled,
    complete,
    setErrorState,
    reset,

    // Helpers
    isActiveMessage,
    isActiveSession,

    // Computed properties
    canSend,
    isSending,
    isStreaming,
    showStopButton,
    canCancel,
    isCancelling,
    hasError,

    // Debug
    transitionLog: transitionLog.current
  };
};
