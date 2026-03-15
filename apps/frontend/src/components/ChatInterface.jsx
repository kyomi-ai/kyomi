// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ChatInterface - Shared chat UI component
 *
 * Used by both Chat.jsx (full page) and DashboardCopilotSidebar (sidebar).
 * Handles the core chat functionality:
 * - Message display with agent thinking
 * - Input area with send/cancel
 * - WebSocket event handling
 * - Streaming state management
 */

import React, { useState, useEffect, useRef, useCallback, memo } from 'react';
import { useAuth } from '../context/AuthContext';
import { useWebSocket } from '../context/WebSocketContext';
import { useCapabilities } from '../context/CapabilitiesContext';
import { useChatState } from '../hooks/useChatState';
import { processThinkingEvent } from '../hooks/useAgentThinking';
import { MarkdownRenderer } from './MarkdownRenderer';
import AgentThinking from './AgentThinking';
import { Alert, AlertDescription } from './ui/alert';

/**
 * Memoized message component
 */
const ChatMessage = memo(({
  message,
  agentThinking,
  isStreaming,
  activeMessageId,
  variant,
  sessionId,
  onMessageUpdate,
  renderMessageActions,
  onWatchApproved,
  acceptedCardIds
}) => {
  const isUser = message.sender === 'user';
  const messageThinking = agentThinking[message.id];
  const shouldShowThinking = (messageThinking?.events?.length > 0) ||
    (!isUser && isStreaming && message.id === activeMessageId);

  // Variant-specific styling
  // NOTE: Assistant message padding must match AgentThinking negative margins (-mx-6 -mt-4)
  const userMessageClass = variant === 'sidebar'
    ? 'max-w-[85%] px-3 py-2 bg-primary text-primary-foreground rounded-2xl text-sm'
    : 'max-w-sm sm:max-w-md lg:max-w-lg xl:max-w-2xl px-4 py-3 bg-primary text-primary-foreground rounded-2xl shadow-sm';

  const assistantMessageClass = variant === 'sidebar'
    ? 'w-full px-6 py-4 bg-muted border border-accent rounded-2xl text-sm overflow-hidden'  // px-6 py-4 to match -mx-6 -mt-4
    : 'w-full px-6 py-4 bg-card border border-accent rounded-2xl shadow-sm overflow-hidden';

  return (
    <div className={`flex flex-col ${isUser ? 'items-end' : 'items-start'}`}>
      <div
        id={`message-${message.id}`}
        className={isUser ? userMessageClass : assistantMessageClass}
      >
        {isUser ? (
          <div className={variant === 'sidebar' ? '' : 'text-sm'}>{message.text}</div>
        ) : (
          <>
            {shouldShowThinking && (
              <AgentThinking
                thinkingEvents={messageThinking?.events}
                isActive={messageThinking?.isActive}
                variant="header-bar"
                tokenUsage={messageThinking?.tokenUsage}
              />
            )}
            {message.text && (
              <MarkdownRenderer
                className=""
                messageId={message.id}
                sessionId={sessionId}
                isStreaming={message.isStreaming}
                onMessageUpdate={onMessageUpdate}
                isChatBubble={variant !== 'sidebar'}
                onWatchApproved={onWatchApproved}
                acceptedCardIds={acceptedCardIds}
              >
                {message.text}
              </MarkdownRenderer>
            )}
          </>
        )}
      </div>
      {/* Render custom message actions if provided */}
      {!isUser && message.text && renderMessageActions && renderMessageActions(message)}
    </div>
  );
}, (prevProps, nextProps) => {
  if (prevProps.message !== nextProps.message) return false;
  const prevThinking = prevProps.agentThinking[prevProps.message.id];
  const nextThinking = nextProps.agentThinking[nextProps.message.id];
  if (prevThinking !== nextThinking) return false;
  const isPrevActive = prevProps.isStreaming && prevProps.message.id === prevProps.activeMessageId;
  const isNextActive = nextProps.isStreaming && nextProps.message.id === nextProps.activeMessageId;
  if (isPrevActive !== isNextActive) return false;
  if (prevProps.sessionId !== nextProps.sessionId) return false;
  // Re-render when renderMessageActions changes (e.g., when preview state updates)
  if (prevProps.renderMessageActions !== nextProps.renderMessageActions) return false;
  if (prevProps.onWatchApproved !== nextProps.onWatchApproved) return false;
  if (prevProps.acceptedCardIds !== nextProps.acceptedCardIds) return false;
  return true;
});

/**
 * ChatInterface component
 *
 * @param {Object} props
 * @param {string} props.variant - 'full' for Chat page, 'sidebar' for Copilot
 * @param {string} props.contextType - Filter WebSocket events (e.g., 'dashboard_copilot')
 * @param {string} props.apiEndpoint - API endpoint for sending messages
 * @param {Object|Function} props.apiPayloadExtras - Extra fields to include in API payload (object or function returning object)
 * @param {string} props.sessionId - Current session ID (optional, for filtering)
 * @param {Function} props.onSessionCreated - Callback when new session is created
 * @param {Function} props.onFirstThinkingEvent - Callback on first thinking event
 * @param {Function} props.renderMessageActions - Render function for message actions
 * @param {Function} props.onMessageUpdate - Callback for message content updates
 * @param {string} props.placeholder - Input placeholder text
 * @param {string} props.emptyStateMessage - Message shown when no messages
 * @param {string} props.emptyStateSubtext - Subtext for empty state
 * @param {React.ReactNode} props.emptyStateContent - Custom empty state content
 * @param {Function} props.onCustomWebSocketEvent - Handle custom WebSocket events
 * @param {Function} props.onWatchApproved - Callback when watch preview is approved (watchData, cardId)
 * @param {Set} props.acceptedCardIds - Set of card IDs that have been accepted
 */
export function ChatInterface({
  variant = 'full',
  contextType = null,
  apiEndpoint = '/api/v1/chat/sessions/message',
  apiPayloadExtras = {},
  sessionId: externalSessionId,
  onSessionCreated,
  onFirstThinkingEvent,
  renderMessageActions,
  onMessageUpdate,
  placeholder = 'Ask me anything...',
  emptyStateMessage = 'Start a conversation',
  emptyStateSubtext,
  emptyStateContent,
  onCustomWebSocketEvent,
  onWatchApproved,
  acceptedCardIds
}) {
  const { apiClient } = useAuth();
  const { subscribe, connectionState, send: sendWebSocketMessage } = useWebSocket();
  const { creditsExhausted, subscriptionTier } = useCapabilities();
  const chatState = useChatState();

  const [messages, setMessages] = useState([]);
  const [inputMessage, setInputMessage] = useState('');
  const [agentThinking, setAgentThinking] = useState({});
  const [internalSessionId, setInternalSessionId] = useState(null);

  const messagesEndRef = useRef(null);
  const textareaRef = useRef(null);
  const sessionIdRef = useRef(null);
  const seenFirstEventRef = useRef({});

  // Use external session ID if provided, otherwise internal
  const sessionId = externalSessionId !== undefined ? externalSessionId : internalSessionId;

  // Keep ref in sync
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  // Clear messages when session is reset (sessionId goes to null from external prop)
  const prevExternalSessionId = useRef(externalSessionId);
  useEffect(() => {
    // If external session was set and is now null, clear everything for fresh start
    if (prevExternalSessionId.current !== null && externalSessionId === null) {
      setMessages([]);
      setAgentThinking({});
      setInternalSessionId(null);
      seenFirstEventRef.current = {};
      chatState.reset();
    }
    prevExternalSessionId.current = externalSessionId;
  }, [externalSessionId, chatState]);

  // Scroll to bottom
  const scrollToBottom = useCallback(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, []);

  useEffect(() => {
    scrollToBottom();
  }, [messages, agentThinking, scrollToBottom]);

  // Auto-focus textarea
  useEffect(() => {
    if (messages.length === 0 && textareaRef.current && chatState.canSend) {
      setTimeout(() => textareaRef.current?.focus(), 100);
    }
  }, [messages.length, chatState.canSend]);

  // Subscribe to WebSocket events
  useEffect(() => {
    // Filter by context_type if specified
    const shouldHandleEvent = (message, eventContextType) => {
      if (contextType && eventContextType !== contextType) return false;
      if (sessionIdRef.current && message.session_id !== sessionIdRef.current) return false;
      return true;
    };

    // Agent thinking events
    const unsubscribeThinking = subscribe('agent_thinking', (message) => {
      const thinkingEvent = message.data?.event;
      const tokenUsage = message.data?.token_usage;
      const eventContextType = thinkingEvent?.context_type;

      if (!shouldHandleEvent(message, eventContextType)) return;
      if (!thinkingEvent || !message.message_id) return;

      // Handle first event
      if (!seenFirstEventRef.current[message.message_id]) {
        seenFirstEventRef.current[message.message_id] = true;

        // Update session ID if needed
        if (!sessionIdRef.current && message.session_id) {
          setInternalSessionId(message.session_id);
          onSessionCreated?.(message.session_id);
        }

        // Transition to streaming
        if (chatState.state === 'sending' && message.message_id) {
          chatState.startStreaming(message.message_id);
        }

        // Create assistant message
        setMessages(prev => {
          if (!prev.find(msg => msg.id === message.message_id)) {
            return [...prev, {
              id: message.message_id,
              text: '',
              sender: 'assistant',
              timestamp: new Date().toISOString(),
              isStreaming: true
            }];
          }
          return prev;
        });

        onFirstThinkingEvent?.(message);
      }

      // Update thinking state
      setAgentThinking(prev => {
        const current = prev[message.message_id] || { events: [], isActive: false, tokenUsage: null };
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
    });

    // Chat stream events
    const unsubscribeStream = subscribe('chat_stream', (message) => {
      const eventContextType = message.data?.context_type;
      if (!shouldHandleEvent(message, eventContextType)) return;

      const content = message.data?.content;
      if (!content || !message.message_id) return;

      setMessages(prev => prev.map(msg => {
        if (msg.id === message.message_id) {
          return { ...msg, text: (msg.text || '') + content, isStreaming: true };
        }
        return msg;
      }));
    });

    // Chat complete events
    const unsubscribeComplete = subscribe('chat_complete', (message) => {
      const eventContextType = message.data?.context_type;
      if (!shouldHandleEvent(message, eventContextType)) return;

      const fullContent = message.data?.content;

      // Ignore if cancelled
      if (chatState.state === 'cancelling' || chatState.state === 'cancelled') return;

      // Complete the chat state
      if (chatState.state === 'sending' || chatState.state === 'streaming') {
        chatState.complete();
      }

      // Update message
      setMessages(prev => prev.map(msg => {
        if (msg.id === message.message_id && msg.sender === 'assistant') {
          return { ...msg, text: fullContent || msg.text, isStreaming: false };
        }
        return msg;
      }));

      // Stop thinking animation
      setAgentThinking(prev => {
        const current = prev[message.message_id];
        if (current && !current.cancelled) {
          return { ...prev, [message.message_id]: { ...current, isActive: false } };
        }
        return prev;
      });
    });

    // Token usage events
    const unsubscribeTokenUsage = subscribe('token_usage_update', (message) => {
      const tokenUpdate = message.data?.token_usage;
      if (!tokenUpdate || !message.message_id) return;
      if (sessionIdRef.current && message.session_id !== sessionIdRef.current) return;

      setAgentThinking(prev => {
        const current = prev[message.message_id];
        if (!current || current.cancelled) return prev;
        return { ...prev, [message.message_id]: { ...current, tokenUsage: tokenUpdate } };
      });
    });

    // Request cancelled events
    const unsubscribeCancelled = subscribe('request_cancelled', (message) => {
      if (chatState.isActiveMessage(message.message_id)) {
        chatState.confirmCancelled();
      }

      setMessages(prev => prev.map(msg => {
        if (msg.id === message.message_id && msg.sender === 'assistant') {
          return { ...msg, text: '_Request cancelled by user._', isStreaming: false, cancelled: true };
        }
        return msg;
      }));

      setAgentThinking(prev => {
        const current = prev[message.message_id];
        if (current) {
          return { ...prev, [message.message_id]: { ...current, isActive: false, cancelled: true } };
        }
        return prev;
      });
    });

    // Error events
    const unsubscribeError = subscribe('error', (message) => {
      const eventContextType = message.data?.context_type;
      if (contextType && eventContextType !== contextType) return;
      chatState.setErrorState(message.data?.message || 'An error occurred');
    });

    // Custom event handler
    let unsubscribeCustom;
    if (onCustomWebSocketEvent) {
      // Let parent handle custom events (like dashboard_update)
      unsubscribeCustom = onCustomWebSocketEvent(subscribe, sessionIdRef);
    }

    return () => {
      unsubscribeThinking();
      unsubscribeStream();
      unsubscribeComplete();
      unsubscribeTokenUsage();
      unsubscribeCancelled();
      unsubscribeError();
      unsubscribeCustom?.();
    };
  }, [subscribe, contextType, chatState, onSessionCreated, onFirstThinkingEvent, onCustomWebSocketEvent]);

  // Helper function to calculate time context for agent awareness
  const getTimeContext = () => {
    const now = new Date();

    // Local time with offset in ISO format (sufficient for UTC conversion)
    // Get timezone offset in minutes and convert to ±HH:MM format
    const offsetMinutes = -now.getTimezoneOffset();
    const offsetHours = Math.floor(Math.abs(offsetMinutes) / 60);
    const offsetMins = Math.abs(offsetMinutes) % 60;
    const offsetSign = offsetMinutes >= 0 ? '+' : '-';
    const offsetStr = `${offsetSign}${String(offsetHours).padStart(2, '0')}:${String(offsetMins).padStart(2, '0')}`;

    // Format local time: YYYY-MM-DDTHH:MM:SS±HH:MM
    const year = now.getFullYear();
    const month = String(now.getMonth() + 1).padStart(2, '0');
    const date = String(now.getDate()).padStart(2, '0');
    const hours = String(now.getHours()).padStart(2, '0');
    const minutes = String(now.getMinutes()).padStart(2, '0');
    const seconds = String(now.getSeconds()).padStart(2, '0');
    const currentTimeUserTz = `${year}-${month}-${date}T${hours}:${minutes}:${seconds}${offsetStr}`;

    return {
      current_time_user_tz: currentTimeUserTz
    };
  };

  // Send message
  const sendMessage = async () => {
    if (!inputMessage.trim() || !chatState.canSend) return;

    if (connectionState !== 'connected') {
      return;
    }

    const userMessage = {
      id: `user-${Date.now()}-${Math.random().toString(36).substring(2, 11)}`,
      text: inputMessage,
      sender: 'user',
      timestamp: new Date().toISOString()
    };

    setMessages(prev => [...prev, userMessage]);
    const currentInput = inputMessage;
    setInputMessage('');

    // Refocus textarea after clearing input
    setTimeout(() => textareaRef.current?.focus(), 0);

    chatState.startSending(sessionIdRef.current);

    try {
      // Calculate current time for agent awareness of relative date queries
      const timeContext = getTimeContext();

      const payload = {
        message: currentInput,
        session_id: sessionIdRef.current,
        current_time_user_tz: timeContext.current_time_user_tz,
        ...apiPayloadExtras
      };

      const response = await apiClient.post(apiEndpoint, payload);

      // Update session ID if new
      if (response.data.session_id && !sessionIdRef.current) {
        setInternalSessionId(response.data.session_id);
        onSessionCreated?.(response.data.session_id);
      }
    } catch (error) {
      setMessages(prev => [...prev, {
        id: `error-${Date.now()}`,
        text: 'Sorry, I encountered an error. Please try again.',
        sender: 'assistant',
        timestamp: new Date().toISOString()
      }]);
      chatState.setErrorState(error.message || 'Failed to send message');
    }
  };

  // Handle enter key
  const handleKeyPress = (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  };

  // Cancel request
  const handleCancel = useCallback(() => {
    const success = chatState.requestCancel();
    if (!success || connectionState !== 'connected') return;

    sendWebSocketMessage({
      type: 'cancel_request',
      message_id: chatState.activeMessageId,
      ...(contextType && { context_type: contextType })
    });
  }, [chatState, connectionState, sendWebSocketMessage, contextType]);

  // Disabled message when credits exhausted
  const aiDisabledMessage = 'AI budget exhausted for this month. Upgrade for more capacity.';

  const isSidebar = variant === 'sidebar';

  return (
    <div className={`flex flex-col h-full min-h-0 ${isSidebar ? '' : 'bg-muted'}`}>
      {/* Messages */}
      <div className={`flex-1 min-h-0 overflow-y-auto ${isSidebar ? 'p-4' : 'p-4 md:p-6'}`}>
        {messages.length === 0 ? (
          emptyStateContent || (
            <div className={`text-center text-muted-foreground text-sm ${isSidebar ? 'py-8' : 'py-16'}`}>
              <p className="mb-2">{emptyStateMessage}</p>
              {emptyStateSubtext && (
                <p className="text-xs text-muted-foreground">{emptyStateSubtext}</p>
              )}
            </div>
          )
        ) : (
          <div className={isSidebar ? 'space-y-4' : 'w-full max-w-full space-y-6'}>
            {messages.map((message) => (
              <ChatMessage
                key={message.id}
                message={message}
                agentThinking={agentThinking}
                isStreaming={chatState.isStreaming}
                activeMessageId={chatState.activeMessageId}
                variant={variant}
                sessionId={sessionId}
                onMessageUpdate={onMessageUpdate}
                renderMessageActions={renderMessageActions}
                onWatchApproved={onWatchApproved}
                acceptedCardIds={acceptedCardIds}
              />
            ))}
          </div>
        )}
        <div ref={messagesEndRef} />
      </div>

      {/* Input area */}
      <div className={`border-t border-border flex-shrink-0 ${isSidebar ? 'p-3' : 'p-4 bg-card'}`}>
        {creditsExhausted ? (
          <div className="text-center text-sm text-muted-foreground py-2">
            {aiDisabledMessage}
          </div>
        ) : (
          <>
            {connectionState !== 'connected' && (
              <div className="mb-2 text-sm text-muted-foreground flex items-center gap-2">
                <div className="w-2 h-2 bg-warning-foreground rounded-full animate-pulse"></div>
                <span>
                  {connectionState === 'connecting' ? 'Connecting...' :
                   connectionState === 'reconnecting' ? 'Reconnecting...' : 'Disconnected'}
                </span>
              </div>
            )}
            <div className="relative flex items-center">
              <textarea
                ref={textareaRef}
                value={inputMessage}
                onChange={(e) => {
                  setInputMessage(e.target.value);
                  e.target.style.height = 'auto';
                  e.target.style.height = Math.min(e.target.scrollHeight, isSidebar ? 120 : 200) + 'px';
                }}
                onKeyPress={handleKeyPress}
                placeholder={placeholder}
                className={`w-full pr-12 resize-none border border-input focus:outline-none focus:ring-2 focus:ring-ring focus:border-transparent bg-background ${
                  isSidebar
                    ? 'rounded-lg px-3 py-2 text-sm min-h-[40px] max-h-[120px]'
                    : 'rounded-xl px-4 py-3 shadow-sm min-h-[52px] max-h-[200px]'
                }`}
                rows={1}
                disabled={!chatState.canSend}
              />
              {chatState.showStopButton ? (
                <button
                  onClick={handleCancel}
                  disabled={!chatState.canCancel}
                  className="absolute right-2 top-2 bottom-2 my-auto px-3 py-2 bg-destructive hover:bg-destructive/90 disabled:bg-muted disabled:cursor-not-allowed text-white rounded-lg transition-colors flex items-center gap-1.5"
                  aria-label="Stop generating"
                  title={chatState.canCancel ? 'Stop generating' : 'Waiting for response...'}
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                  </svg>
                  {!isSidebar && <span className="text-sm font-medium">Stop</span>}
                </button>
              ) : (
                <button
                  onClick={sendMessage}
                  disabled={!chatState.canSend || !inputMessage.trim() || connectionState !== 'connected'}
                  className="absolute right-2 top-2 bottom-2 my-auto p-2 bg-primary text-primary-foreground rounded-lg hover:opacity-90 disabled:bg-muted disabled:cursor-not-allowed transition-opacity flex items-center justify-center"
                  aria-label="Send message"
                  title={connectionState !== 'connected' ? 'Waiting for connection...' : 'Send message'}
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
                  </svg>
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

// Export for external use
export { ChatMessage };
export default ChatInterface;
