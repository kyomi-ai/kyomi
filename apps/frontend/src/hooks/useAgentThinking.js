// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Agent Thinking Hook
 *
 * Manages agent thinking events state and WebSocket subscriptions.
 * Used by both Chat.jsx and DashboardCopilotSidebar.jsx to avoid
 * duplicating the event handling logic.
 *
 * Features:
 * - Subscribes to agent_thinking WebSocket events
 * - Handles is_update flag for in-place tool event updates
 * - Filters events by context_type and session_id
 * - Manages token usage tracking
 */

import { useState, useCallback, useEffect, useRef } from 'react';
import { useWebSocket } from '../context/WebSocketContext';

/**
 * Process a thinking event, handling updates to existing events
 *
 * Deduplicates by event_id (all events now have unique IDs).
 * Maintains chronological sort order using lexicographic comparison
 * (event_id format: {unix_millis}-{counter} ensures sort order).
 *
 * @param {Array} existingEvents - Current events array
 * @param {Object} newEvent - New event to add or update
 * @returns {Array} Updated events array
 */
export function processThinkingEvent(existingEvents, newEvent) {
  // Dedupe by event_id (all events now have unique IDs)
  const existingIndex = existingEvents.findIndex(e => e.event_id === newEvent.event_id);

  if (existingIndex !== -1) {
    // Event exists - update it in place
    const updated = [...existingEvents];
    updated[existingIndex] = newEvent;
    return updated;
  }

  // New event - add and maintain sort order
  const updated = [...existingEvents, newEvent];
  updated.sort((a, b) => a.event_id.localeCompare(b.event_id));
  return updated;
}

/**
 * Hook for managing agent thinking state
 *
 * @param {Object} options
 * @param {string} options.contextType - Filter events by context_type (e.g., 'dashboard_copilot')
 * @param {React.RefObject} options.sessionIdRef - Ref to current session ID for filtering
 * @param {Function} options.onFirstEvent - Callback when first event arrives (for state transitions)
 * @returns {Object} Thinking state and controls
 */
export function useAgentThinking({ contextType = null, sessionIdRef, onFirstEvent } = {}) {
  const { subscribe } = useWebSocket();
  const [agentThinking, setAgentThinking] = useState({});

  // Track if we've seen the first event for a message (for onFirstEvent callback)
  const seenFirstEventRef = useRef({});

  /**
   * Handle an incoming thinking event
   */
  const handleThinkingEvent = useCallback((message) => {
    const thinkingEvent = message.data?.event;
    const tokenUsage = message.data?.token_usage;

    if (!thinkingEvent || !message.message_id) return;

    // Filter by context_type if specified
    // Note: context_type is inside the event object for agent_thinking
    if (contextType && thinkingEvent.context_type !== contextType) return;

    // Filter by session if sessionIdRef provided
    if (sessionIdRef?.current && message.session_id !== sessionIdRef.current) return;

    // Call onFirstEvent callback for first event of this message
    if (onFirstEvent && !seenFirstEventRef.current[message.message_id]) {
      seenFirstEventRef.current[message.message_id] = true;
      onFirstEvent(message);
    }

    // Update thinking state
    setAgentThinking(prev => {
      const current = prev[message.message_id] || { events: [], isActive: false, tokenUsage: null };

      // Don't process if message was cancelled
      if (current.cancelled) return prev;

      const updatedEvents = processThinkingEvent(current.events, thinkingEvent);

      return {
        ...prev,
        [message.message_id]: {
          events: updatedEvents,
          isActive: true,
          tokenUsage: tokenUsage || current.tokenUsage
        }
      };
    });
  }, [contextType, sessionIdRef, onFirstEvent]);

  /**
   * Mark thinking as complete for a message
   */
  const completeThinking = useCallback((messageId) => {
    setAgentThinking(prev => {
      const current = prev[messageId];
      if (!current) return prev;
      return {
        ...prev,
        [messageId]: { ...current, isActive: false }
      };
    });
  }, []);

  /**
   * Mark thinking as cancelled for a message
   */
  const cancelThinking = useCallback((messageId) => {
    setAgentThinking(prev => {
      const current = prev[messageId];
      if (!current) return prev;
      return {
        ...prev,
        [messageId]: { ...current, isActive: false, cancelled: true }
      };
    });
  }, []);

  /**
   * Clear thinking state (e.g., when switching sessions)
   */
  const clearThinking = useCallback(() => {
    setAgentThinking({});
    seenFirstEventRef.current = {};
  }, []);

  /**
   * Get thinking state for a specific message
   */
  const getThinkingForMessage = useCallback((messageId) => {
    return agentThinking[messageId] || { events: [], isActive: false, tokenUsage: null };
  }, [agentThinking]);

  // Subscribe to WebSocket events
  useEffect(() => {
    const unsubscribe = subscribe('agent_thinking', handleThinkingEvent);
    return unsubscribe;
  }, [subscribe, handleThinkingEvent]);

  return {
    agentThinking,
    setAgentThinking,
    completeThinking,
    cancelThinking,
    clearThinking,
    getThinkingForMessage,
    handleThinkingEvent
  };
}

export default useAgentThinking;
