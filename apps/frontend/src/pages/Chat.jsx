// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useRef, useCallback, memo } from 'react';
import { useNavigate, useLocation, useParams, useSearchParams } from 'react-router-dom';
import { useAuth } from '../context/AuthContext';
import { useWebSocket } from '../context/WebSocketContext';
import { useCapabilities } from '../context/CapabilitiesContext';
import { useChatState } from '../hooks/useChatState';
import { processThinkingEvent } from '../hooks/useAgentThinking';
import { MarkdownRenderer } from '../components/MarkdownRenderer';
import AgentThinking from '../components/AgentThinking';
import SaveDashboardModal from '../components/SaveDashboardModal';
import ChartInfoModal from '../components/ChartInfoModal';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Tooltip, TooltipTrigger, TooltipContent } from '../components/ui/tooltip';
import { Badge } from '../components/ui/badge';
import { Spinner } from '../components/ui/spinner';
import { formatRelativeTime } from '../lib/formatters';
import InlineEditableTitle from '../components/InlineEditableTitle';
import { useProductTour } from '../components/ProductTour';
import useDatasources from '../hooks/useDatasources';
import NoDatasourcesEmptyState from '../components/NoDatasourcesEmptyState';
import { useSystemConfig } from '../context/SystemConfigContext';
import { trackEvent } from '../utils/analytics';
import { toast } from '@/lib/toast';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '../components/ui/dropdown-menu';
import ConfirmDialog from '../components/ConfirmDialog';
import useConfirm from '../hooks/useConfirm';

// Memoized message component to prevent re-renders when parent re-renders
const ChatMessage = memo(({
  message,
  agentThinking,
  isStreaming,
  activeMessageId,
  currentSessionId,
  sessionMetadata,
  currentUser,
  onTogglePin,
  onOpenDashboardModal,
  onMessageUpdate,
  onSaveChartToDashboard,
  onShowChartInfo,
  isTrialMode = false
}) => {
  // Determine if this message is from the current user (for positioning)
  // In shared conversations, check sent_by.user_id
  // In private conversations, sent_by is null/undefined, so all user messages are "mine"
  const isMyMessage = message.sender === 'user' &&
    (!message.sent_by || message.sent_by.user_id === currentUser?.user_id);

  // Always show sender name for consistency
  // For user messages: use sent_by if available (shared conversations), otherwise use current user
  // For assistant messages: always show "Kyomi"
  const senderName = message.sender === 'assistant'
    ? 'Kyomi'
    : (message.sent_by?.display_name || currentUser?.name || currentUser?.email || 'You');

  return (
    <div
      key={message.id}
      className={`flex flex-col ${isMyMessage ? 'items-end' : 'items-start'}`}
    >
      <div
        id={`message-${message.id}`}
        className={`${
          isMyMessage
            ? 'max-w-sm sm:max-w-md lg:max-w-lg xl:max-w-2xl px-4 py-3 text-primary-foreground bg-primary rounded-2xl shadow-sm'
            : 'w-full px-6 py-4 bg-card border border-border rounded-2xl shadow-sm overflow-hidden'
        }`}
      >
        {isMyMessage ? (
          <div className="text-sm">
            {message.text}
          </div>
        ) : (
          <>
            {/* Show live agent thinking during and after streaming */}
            {(() => {
              const messageThinking = agentThinking[message.id];
              // Show if: 1) we have thinking data, OR 2) this is the active assistant message being processed
              const hasThinkingData = messageThinking && messageThinking.events && messageThinking.events.length > 0;
              const isActiveMessage = message.sender === 'assistant' && isStreaming && message.id === activeMessageId;
              const shouldShow = hasThinkingData || isActiveMessage;
              return shouldShow;
            })() && (
              <AgentThinking
                thinkingEvents={agentThinking[message.id]?.events}
                isActive={agentThinking[message.id]?.isActive}
                variant="header-bar"
                tokenUsage={agentThinking[message.id]?.tokenUsage}
              />
            )}

            {/* Show response text */}
            {message.text && (
              <MarkdownRenderer
                className="text-sm"
                messageId={message.id}
                sessionId={currentSessionId}
                isStreaming={message.isStreaming}
                onMessageUpdate={onMessageUpdate}
                onSaveChartToDashboard={onSaveChartToDashboard}
                onShowChartInfo={onShowChartInfo}
                isChatBubble={true}
                isTrialMode={isTrialMode}
              >
                {message.text}
              </MarkdownRenderer>
            )}
          </>
        )}
      </div>

      {/* Sender name + timestamp + actions for assistant messages */}
      {message.sender === 'assistant' && message.text && (
        <div className="w-full flex items-center justify-between gap-2 mt-1 px-1">
          <div className="text-xs text-muted-foreground">
            {senderName} · {message.timestamp && formatRelativeTime(message.timestamp)}
          </div>
          <div className="flex items-center gap-3">
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => onTogglePin(message.id)}
                  className={`text-xs transition-colors flex items-center gap-1 ${
                    message.pinned
                      ? 'text-primary hover:text-primary/80'
                      : 'text-muted-foreground hover:text-foreground'
                  }`}
                  aria-label={message.pinned ? 'Unpin message' : 'Pin message'}
                >
                  <svg className="w-4 h-4" fill={message.pinned ? 'currentColor' : 'none'} stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                  </svg>
                </button>
              </TooltipTrigger>
              <TooltipContent>{message.pinned ? 'Unpin message' : 'Pin message'}</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => onOpenDashboardModal(message.text, message.id)}
                  className="text-xs text-muted-foreground hover:text-primary transition-colors flex items-center gap-1"
                  aria-label="Save to Dashboard"
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 7H5a2 2 0 00-2 2v9a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-3m-1 4l-3 3m0 0l-3-3m3 3V4" />
                  </svg>
                  <span>Save to Dashboard</span>
                </button>
              </TooltipTrigger>
              <TooltipContent>Save to Dashboard</TooltipContent>
            </Tooltip>
          </div>
        </div>
      )}

      {/* Sender name + timestamp for user messages (mine and others in shared conversations) */}
      {message.sender === 'user' && (
        <div className={`text-xs text-muted-foreground mt-1 px-1 ${isMyMessage ? 'text-right' : 'text-left'}`}>
          {senderName} · {message.timestamp && formatRelativeTime(message.timestamp)}
        </div>
      )}
    </div>
  );
}, (prevProps, nextProps) => {
  // Custom comparison function for memo
  // Return true if props are equal (skip re-render), false if different (do re-render)

  // Check if message object changed
  if (prevProps.message !== nextProps.message) return false;

  // Check if THIS message's thinking data changed
  const prevThinking = prevProps.agentThinking[prevProps.message.id];
  const nextThinking = nextProps.agentThinking[nextProps.message.id];
  if (prevThinking !== nextThinking) return false;

  // Check if processing state changed for THIS message
  const isPrevActiveMessage = prevProps.isStreaming && prevProps.message.id === prevProps.activeMessageId;
  const isNextActiveMessage = nextProps.isStreaming && nextProps.message.id === nextProps.activeMessageId;
  if (isPrevActiveMessage !== isNextActiveMessage) return false;

  // Check if session changed
  if (prevProps.currentSessionId !== nextProps.currentSessionId) return false;

  // Check if session metadata changed (affects sender display)
  if (prevProps.sessionMetadata.shared !== nextProps.sessionMetadata.shared) return false;

  // Callbacks should be stable due to useCallback
  if (prevProps.onTogglePin !== nextProps.onTogglePin) return false;
  if (prevProps.onOpenDashboardModal !== nextProps.onOpenDashboardModal) return false;
  if (prevProps.onMessageUpdate !== nextProps.onMessageUpdate) return false;
  if (prevProps.onSaveChartToDashboard !== nextProps.onSaveChartToDashboard) return false;
  if (prevProps.onShowChartInfo !== nextProps.onShowChartInfo) return false;

  // All relevant props are equal, skip re-render
  return true;
});


const Chat = () => {
  const navigate = useNavigate();
  const location = useLocation();
  const { sessionId: urlSessionId } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const capabilities = useCapabilities();

  // Chat state machine - replaces isLoading, isProcessing, aiMessageIdRef
  const chatState = useChatState();

  const [messages, setMessages] = useState([]);
  const [inputMessage, setInputMessage] = useState('');
  const [currentSessionId, setCurrentSessionId] = useState(null);
  const [currentGreeting, setCurrentGreeting] = useState('');
  const [sessionTitle, setSessionTitle] = useState('');
  const [sessionMetadata, setSessionMetadata] = useState({ shared: false, created_by: null, slack_channel_id: null }); // Session sharing and Slack sync metadata
  const [agentThinking, setAgentThinking] = useState({}); // Store thinking events by message ID
  const [thinkingEventBuffer, setThinkingEventBuffer] = useState(() => {
    return [];
  }); // Buffer for events received before session_id known
  const [isLoadingSession, setIsLoadingSession] = useState(false); // For loading session messages
  const [dashboardModal, setDashboardModal] = useState({ isOpen: false, messageContent: '', messageId: '' });
  const [chartInfoModal, setChartInfoModal] = useState({ isOpen: false, spec: null });
  const [showPinnedOnly, setShowPinnedOnly] = useState(false); // Filter to show only pinned messages
  const [skipAiResponse, setSkipAiResponse] = useState(false); // For shared conversations - post comment without AI response
  const messagesEndRef = useRef(null);
  const messagesContainerRef = useRef(null);
  const textareaRef = useRef(null);
  // Use a ref to track current session ID to avoid closure issues
  const currentSessionIdRef = useRef(null);
  // Track when we're creating a brand new session (don't load from DB)
  const isCreatingNewSessionRef = useRef(false);
  // Track if user has sent their first message (for analytics)
  const hasTrackedFirstMessage = useRef(false);
  // Store chart context for "Ask about this chart" feature - cleared after first message
  const chartContextRef = useRef(null);
  const { user, apiClient } = useAuth();
  const { subscribe, connectionState, send: sendWebSocketMessage } = useWebSocket();
  const { creditsExhausted, subscriptionTier, loading: capabilitiesLoading, refetch: refetchCapabilities } = useCapabilities();
  const { isOpen: confirmDialogOpen, dialogProps: confirmDialogProps, confirm } = useConfirm();

  // Datasources check for empty state
  const { hasDatasources, loading: datasourcesLoading } = useDatasources();
  const { isPersonalMode, llmConfigured } = useSystemConfig();

  // Product tour
  const { showTour } = useProductTour();

  // Disabled message when credits exhausted
  const aiDisabledMessage = 'AI budget exhausted for this month. Upgrade for more capacity.';

  // Helper function to update both session ID state and ref
  const updateCurrentSessionId = (newSessionId) => {
    setCurrentSessionId(newSessionId);
    currentSessionIdRef.current = newSessionId;
  };
  // Note: OAuth reconnection dialog has been moved to AppWithOAuthBar component
  // It now appears application-wide, not just in Chat

  // Track if we're currently scrolling to prevent stutter
  const scrollTimeoutRef = useRef(null);

  // Track if we've already triggered the first chart tour in this session to prevent duplicates
  const firstChartTourTriggeredRef = useRef(false);

  // Check if user is scrolled near the bottom (Slack-style behavior)
  const isNearBottom = useCallback(() => {
    const container = messagesContainerRef.current;
    if (!container) return true; // Default to true if container not found

    const threshold = 100; // pixels from bottom
    const { scrollTop, scrollHeight, clientHeight } = container;
    return scrollHeight - scrollTop - clientHeight < threshold;
  }, []);

  const scrollToBottom = useCallback(() => {
    // Cancel any pending scroll and schedule a new one
    if (scrollTimeoutRef.current) {
      clearTimeout(scrollTimeoutRef.current);
    }

    // Only scroll if user is already near the bottom (Slack-style smart scroll)
    if (!isNearBottom()) {
      return;
    }

    // Debounce scroll requests - only scroll after 50ms of no new requests
    scrollTimeoutRef.current = setTimeout(() => {
      messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }, 50);
  }, [isNearBottom]);

  // Generate a random greeting
  const generateGreeting = () => {
    const greetings = [
      "Ready to dive into the data, {name}?",
      "What patterns shall we uncover today, {name}?", 
      "Which datasets are calling your attention, {name}?",
      "What story do you think the numbers will tell us, {name}?",
      "Let's turn your data into decisions, {name}!",
      "Ready to crunch some numbers, {name}?",
      "What metrics should we examine today, {name}?",
      "Time to unlock some data-driven insights, {name}!",
      "What hidden trends can we discover, {name}?",
      "Ready to explore the data landscape, {name}?",
      "What analytics adventure awaits us, {name}?",
      "Let's dig deeper into your data, {name}!",
      "What analysis can I help you with today, {name}?",
      "Ready to transform data into actionable insights, {name}?",
      "What questions should we ask the data, {name}?",
      "Let's make sense of your numbers, {name}!",
      "What's the data puzzle we're solving today, {name}?",
      "Ready to connect the dots in your data, {name}?",
      "What trends are you curious about, {name}?",
      "Let's see what the data reveals, {name}!"
    ];
    const randomGreeting = greetings[Math.floor(Math.random() * greetings.length)];
    return randomGreeting.replace('{name}', user?.name?.split(' ')[0] || 'Jason');
  };

  useEffect(() => {
    scrollToBottom();
  }, [messages, agentThinking, scrollToBottom]);

  // Cleanup scroll timeout on unmount
  useEffect(() => {
    return () => {
      if (scrollTimeoutRef.current) {
        clearTimeout(scrollTimeoutRef.current);
      }
    };
  }, []);

  // Debug: Log when sessionTitle state changes
  useEffect(() => {
  }, [sessionTitle]);


  // Subscribe to WebSocket events
  useEffect(() => {

    // Subscribe to session_created events
    const unsubscribeSessionCreated = subscribe('session_created', (message) => {
      // If we don't have a session ID yet, this MIGHT be OUR new session
      // But only navigate if we're in SENDING state (we just sent a message)
      // If we're IDLE, user clicked "New Chat" and we should ignore old session events
      if (!currentSessionIdRef.current && message.session_id && message.data) {
        if (chatState.state === 'sending') {
          // Only navigate - let the useEffect handle updating currentSessionId
          // This way the useEffect can detect it's a new session creation
          if (window.location.pathname !== `/chat/${message.session_id}`) {
            navigate(`/chat/${message.session_id}`, { replace: true });
          }
          // Only set title if it exists (null title means it will arrive later via title_update)
          if (message.data.title) {
            setSessionTitle(message.data.title);
          } else {
          }
          // Set session metadata from backend data
          setSessionMetadata({
            shared: message.data.shared || false,
            created_by: message.data.created_by || null,
            slack_channel_id: message.data.slack_channel_id || null
          });
        } else {
        }
      }
    });

    // Subscribe to title_update events
    const unsubscribeTitleUpdate = subscribe('title_update', (message) => {
      if (message.session_id && message.data?.title) {
        if (currentSessionIdRef.current === message.session_id) {
          setSessionTitle(message.data.title);
        } else {
        }
      }
    });

    // Subscribe to agent_thinking events
    const unsubscribeAgentThinking = subscribe('agent_thinking', (message) => {
      const thinkingEvent = message.data?.event;
      const tokenUsage = message.data?.token_usage;

      if (thinkingEvent && message.session_id && message.message_id) {
        // CRITICAL: Ignore events from other sessions (prevents chat bleed)
        // BUT: Allow events when currentSession is null (new chat race condition)
        // - When starting a new chat, thinking events arrive before API response sets session_id
        // - We need to accept these events and buffer them until API response arrives with full context
        if (currentSessionIdRef.current !== null && message.session_id !== currentSessionIdRef.current) {
          return;
        }

        // If this is a new chat (currentSession is null), buffer the event
        // The API response will merge buffered events with the thinking_events snapshot
        if (currentSessionIdRef.current === null && chatState.state === 'sending') {
          // Create assistant message placeholder (needed for thinking tracker to display)
          setMessages(prev => {
            const existingMessage = prev.find(msg => msg.id === message.message_id);
            if (!existingMessage) {
              return [...prev, {
                id: message.message_id,
                text: '',
                sender: 'assistant',
                isStreaming: true,
                timestamp: message.timestamp || new Date().toISOString(),
                session_id: message.session_id
              }];
            }
            return prev;
          });

          // Transition to streaming when first thinking event arrives
          if (chatState.state === 'sending' && message.message_id) {
            chatState.startStreaming(message.message_id);
          }

          // Buffer the event for API response merge
          setThinkingEventBuffer(prev => {
            const newBuffer = [...prev, { event: thinkingEvent, tokenUsage, messageId: message.message_id }];
            return newBuffer;
          });

          // ALSO add to agentThinking state so it displays immediately
          setAgentThinking(prev => {
            const current = prev[message.message_id] || { events: [], isActive: false, tokenUsage: null };
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

          return; // Buffered and displayed - done
        } else if (currentSessionIdRef.current === null) {
          return;
        }

        // Transition to STREAMING state when first thinking event arrives with message_id
        // This allows cancellation to work (needs message_id)
        if (chatState.state === 'sending' && message.message_id) {
          chatState.startStreaming(message.message_id);
        }

        // Create assistant message immediately if it doesn't exist yet
        setMessages(prev => {
          const existingMessage = prev.find(msg => msg.id === message.message_id);
          if (!existingMessage) {
            return [...prev, {
              id: message.message_id,
              text: '',
              sender: 'assistant',
              isStreaming: true,
              timestamp: message.timestamp || new Date().toISOString(),
              session_id: message.session_id
            }];
          }
          return prev;
        });


        setAgentThinking(prev => {
          const current = prev[message.message_id] || { events: [], isActive: false, tokenUsage: null };

          // Don't restart animation if message was cancelled
          if (current.cancelled) {
            return prev;
          }

          // Use shared utility for event processing (handles is_update logic)
          const updatedEvents = processThinkingEvent(current.events, thinkingEvent);

          // Show thinking tour on first thinking event
          if (current.events.length === 0 && updatedEvents.length === 1) {
            showTour('agentThinking');
          }

          const newState = {
            ...prev,
            [message.message_id]: {
              events: updatedEvents,
              isActive: true,
              tokenUsage: tokenUsage || current.tokenUsage
            }
          };
          return newState;
        });
      }
    });

    // Subscribe to token_usage_update events
    const unsubscribeTokenUsage = subscribe('token_usage_update', (message) => {
      const tokenUpdate = message.data?.token_usage;
      if (tokenUpdate && message.session_id && message.message_id) {
        // CRITICAL: Ignore events from other sessions (prevents chat bleed)
        // Allow events when currentSession is null (new chat race condition)
        if (currentSessionIdRef.current !== null && message.session_id !== currentSessionIdRef.current) {
          return;
        }

        setAgentThinking(prev => {
          const current = prev[message.message_id] || { events: [], isActive: false, tokenUsage: null };

          // Don't update if message was cancelled (avoid triggering re-renders)
          if (current.cancelled) {
            return prev;
          }

          return {
            ...prev,
            [message.message_id]: {
              ...current,
              tokenUsage: tokenUpdate
            }
          };
        });
      }
    });

    // Subscribe to chat_stream events
    const unsubscribeChatStream = subscribe('chat_stream', (message) => {
      const content = message.data?.content;

      if (content && message.session_id && message.message_id) {
        // CRITICAL: Ignore events from other sessions (prevents chat bleed)
        // Allow events when currentSession is null (new chat race condition)
        if (currentSessionIdRef.current !== null && message.session_id !== currentSessionIdRef.current) {
          return;
        }

        setMessages(prevMessages => {
          const messageIndex = prevMessages.findIndex(
            msg => msg.id === message.message_id && msg.sender === 'assistant'
          );

          if (messageIndex !== -1) {
            // Create new array with updated message object (immutable update)
            const updatedMessages = [...prevMessages];
            updatedMessages[messageIndex] = {
              ...updatedMessages[messageIndex],
              text: updatedMessages[messageIndex].text + content
            };
            return updatedMessages;
          } else {
            // Create new assistant message
            return [...prevMessages, {
              id: message.message_id,
              sender: 'assistant',
              text: content,
              timestamp: message.timestamp || new Date().toISOString(),
              session_id: message.session_id,
              isStreaming: true
            }];
          }
        });
      } else {
      }
    });

    // Subscribe to chat_complete events
    const unsubscribeChatComplete = subscribe('chat_complete', (message) => {
      const fullContent = message.data?.content;
      const model = message.data?.model;
      const usage = message.data?.usage;

      // Only process if this message belongs to the active session
      if (message.session_id && message.message_id) {
        // CRITICAL: Ignore events from other sessions (prevents chat bleed)
        // Allow events when currentSession is null (new chat race condition)
        if (currentSessionIdRef.current !== null && message.session_id !== currentSessionIdRef.current) {
          return;
        }

        // Check if this is for the current active session (not just active message)
        // This handles the race condition where chat_complete arrives before startStreaming() is called

        // Ignore chat_complete if we're in cancelling/cancelled state
        // The error message from backend should not overwrite our cancellation message
        if (chatState.state === 'cancelling' || chatState.state === 'cancelled') {
          return;
        }

        if (message.session_id === currentSessionIdRef.current &&
            (chatState.state === 'sending' || chatState.state === 'streaming')) {
          chatState.complete();
        } else {
        }

        setMessages(prevMessages => {
          return prevMessages.map(msg => {
            if (msg.id === message.message_id && msg.sender === 'assistant') {
              return {
                ...msg,
                text: fullContent || msg.text,
                model: model,
                usage: usage,
                isStreaming: false
              };
            }
            return msg;
          });
        });

        // Show tour after first message with a chart (only once per session to prevent duplicates)
        if (fullContent && fullContent.includes('```chartml') && !firstChartTourTriggeredRef.current) {
          firstChartTourTriggeredRef.current = true;
          showTour('firstChart', message.message_id);
        }

        // Only stop thinking animation if request wasn't cancelled
        // Check the cancelled flag to avoid restarting animation after cancellation
        setAgentThinking(prev => {
          const current = prev[message.message_id];
          if (current && !current.cancelled) {
            const updated = {
              ...prev,
              [message.message_id]: {
                ...current,
                isActive: false
              }
            };
            return updated;
          } else if (current?.cancelled) {
          }
          return prev;
        });
      }
    });

    // Subscribe to error events
    const unsubscribeError = subscribe('error', async (message) => {
      const errorMessage = message.data?.message;
      const errorCode = message.data?.code;

      // If error is about AI budget exhausted, refresh capabilities to disable UI
      if (errorCode === 403 || errorMessage?.toLowerCase().includes('ai features are not enabled') ||
          errorMessage?.toLowerCase().includes('budget exhausted') ||
          errorMessage?.toLowerCase().includes('credits exhausted')) {
        await refetchCapabilities();
      }

      setIsLoadingSession(false);
    });

    // Subscribe to request cancellation confirmations
    const unsubscribeRequestCancelled = subscribe('request_cancelled', (message) => {
      const messageId = message.message_id;

      // Check if this is for the current active message
      if (chatState.isActiveMessage(messageId)) {
        chatState.confirmCancelled();
      }

      // Update the assistant message to show it was cancelled
      setMessages(prevMessages => {
        return prevMessages.map(msg => {
          if (msg.id === messageId && msg.sender === 'assistant') {
            return {
              ...msg,
              text: '_Request cancelled by user._',
              isStreaming: false,
              cancelled: true  // Mark as cancelled so chat_complete knows to ignore it
            };
          }
          return msg;
        });
      });

      // Clear agent thinking state (and mark as cancelled)
      setAgentThinking(prev => {
        const current = prev[messageId];
        if (current) {
          return {
            ...prev,
            [messageId]: {
              ...current,
              isActive: false,
              cancelled: true  // Mark as cancelled so chat_complete knows to ignore it
            }
          };
        }
        return prev;
      });
    });

    // Subscribe to shared chat messages for real-time collaboration
    const unsubscribeSharedMessage = subscribe('shared_chat_message', (message) => {
      // Only process if this message is for the current session
      if (message.session_id !== currentSessionIdRef.current) {
        return;
      }

      // The actual message is in the data field
      const incomingMessage = message.data;

      if (!incomingMessage) {
        return;
      }

      // Check if message already exists (avoid duplicates)
      setMessages(prevMessages => {
        // If client_msg_id is provided, check if we already have this message as an optimistic update
        // This handles the race condition where WebSocket arrives before API response
        if (incomingMessage.client_msg_id) {
          const optimisticIndex = prevMessages.findIndex(msg => msg.id === incomingMessage.client_msg_id);
          if (optimisticIndex !== -1) {
            // Update the optimistic message with the real message_id
            return prevMessages.map(msg =>
              msg.id === incomingMessage.client_msg_id
                ? { ...msg, id: incomingMessage.message_id }
                : msg
            );
          }
        }

        // Fall back to checking by message_id (for messages without client_msg_id or from other users)
        const exists = prevMessages.some(msg => msg.id === incomingMessage.message_id);
        if (exists) {
          return prevMessages;
        }

        // Add the new message to the list
        const newMessage = {
          id: incomingMessage.message_id,
          sender: incomingMessage.type,
          text: incomingMessage.content,
          timestamp: incomingMessage.timestamp,
          sent_by: incomingMessage.sent_by,
          isStreaming: false,
        };

        return [...prevMessages, newMessage];
      });
    });

    // Cleanup - unsubscribe from all events on unmount
    return () => {
      unsubscribeSessionCreated();
      unsubscribeTitleUpdate();
      unsubscribeAgentThinking();
      unsubscribeTokenUsage();
      unsubscribeChatStream();
      unsubscribeChatComplete();
      unsubscribeError();
      unsubscribeRequestCancelled();
      unsubscribeSharedMessage();
    };
  }, [subscribe, currentSessionId, navigate, chatState]);

  // Generate greeting when there are no messages and user is available
  useEffect(() => {
    if (messages.length === 0 && user && !currentGreeting) {
      setCurrentGreeting(generateGreeting());
    }
  }, [messages.length, user, currentGreeting]);

  // Track when user lands on /chat page (only once per component mount)
  useEffect(() => {
    trackEvent('chat_page_visited');
  }, []); // Empty dependency array = run once on mount

  // Auto-focus textarea when there are no messages (new chat screen)
  useEffect(() => {
    if (messages.length === 0 && textareaRef.current && chatState.canSend) {
      const timer = setTimeout(() => {
        textareaRef.current?.focus();
      }, 100); // Small delay to ensure DOM is ready
      return () => clearTimeout(timer);
    }
  }, [messages.length, chatState.canSend]);

  // Load session from URL parameter
  useEffect(() => {
    if (urlSessionId && urlSessionId !== currentSessionId) {
      // Don't load if we just created this session - data arrives via WebSocket
      if (isCreatingNewSessionRef.current) {
        updateCurrentSessionId(urlSessionId);
        isCreatingNewSessionRef.current = false;
      } else {
        // Load from DB - user navigated to an existing session
        chatState.reset('switching to different session');
        switchToSession(urlSessionId);
      }
    } else if (!urlSessionId && currentSessionId) {
      // User navigated to /chat (new chat) - clear current session
      chatState.reset('starting new chat');
      updateCurrentSessionId(null);
      setMessages([]);
      setSessionTitle('');
      setSessionMetadata({ shared: false, created_by: null, slack_channel_id: null });
      setCurrentGreeting(generateGreeting());
      isCreatingNewSessionRef.current = false;
    }
  }, [urlSessionId, currentSessionId]);

  // Handle "Ask about this chart" navigation from dashboards
  // When user clicks the chat button on a chart, we receive the chart context via location.state
  useEffect(() => {
    if (location.state?.exploreChart && location.state?.chartMarkdown) {
      const { chartMarkdown, chartTitle } = location.state;

      // Store chart context for prepending to first user message
      // This ensures the backend has the full context when the user asks their question
      chartContextRef.current = chartMarkdown;

      // Create an initial assistant message with the chart (local display only)
      const initialMessage = {
        id: `chart-context-${Date.now()}`,
        text: `I'm ready to help you explore this chart:\n\n${chartMarkdown}\n\nWhat would you like to know about it?`,
        sender: 'assistant',
        timestamp: new Date().toISOString(),
        isChartContext: true // Mark as chart context for potential special handling
      };

      setMessages([initialMessage]);
      setSessionTitle(chartTitle ? `Exploring: ${chartTitle}` : 'Chart Exploration');
      setCurrentGreeting(''); // Clear greeting since we have chart context

      // Clear location state to prevent re-triggering on page refresh
      // Use replace to avoid adding to history
      navigate(location.pathname, { replace: true, state: {} });
    }
  }, [location.state, navigate, location.pathname]);

  // Handle "Continue in Kyomi" deep-link from MCP chart app
  // When a user clicks the chat icon on a chart rendered in Claude.ai,
  // they arrive at /chat?chart=<contextId>. We fetch the stored context
  // from Redis and bootstrap the conversation just like exploreChart above.
  useEffect(() => {
    const chartId = searchParams.get('chart');
    if (!chartId) return;

    let cancelled = false;

    (async () => {
      try {
        const response = await apiClient.get(`/api/v1/chart-context/${chartId}`);
        if (cancelled) return;

        const { chartMarkdown, title } = response.data;

        // Store chart context for prepending to first user message
        chartContextRef.current = chartMarkdown;

        // Create an initial assistant message with the chart
        const initialMessage = {
          id: `chart-context-${Date.now()}`,
          text: `I'm ready to help you explore this chart:\n\n${chartMarkdown}\n\nWhat would you like to know about it?`,
          sender: 'assistant',
          timestamp: new Date().toISOString(),
          isChartContext: true,
        };

        setMessages([initialMessage]);
        setSessionTitle(title ? `Exploring: ${title}` : 'Chart Exploration');
        setCurrentGreeting('');
      } catch (err) {
        if (cancelled) return;

        const initialMessage = {
          id: `chart-context-error-${Date.now()}`,
          text: 'This chart link has expired or is no longer available. You can still ask me anything about your data!',
          sender: 'assistant',
          timestamp: new Date().toISOString(),
        };

        setMessages([initialMessage]);
        setCurrentGreeting('');
      }

      // Clear the query parameter to prevent re-triggering on refresh
      setSearchParams({}, { replace: true });
    })();

    return () => { cancelled = true; };
  }, []); // Run once on mount — deep-link is a one-shot action

  // Handle "Create Watch" navigation from Watches page
  // When user clicks Create Watch, we launch a chat to guide them through setup
  useEffect(() => {
    if (location.state?.createWatch) {
      // Create an initial assistant message to guide watch creation
      const initialMessage = {
        id: `watch-create-${Date.now()}`,
        text: `I can help you set up a watch to monitor your data. What would you like me to keep an eye on?

For example, you could say:
- "Alert me if daily revenue drops more than 10%"
- "Watch for unusual spikes in error rates"
- "Monitor our conversion rate and tell me if it changes significantly"
- "Check our inventory levels daily and warn me if anything is running low"

Just describe what you want to monitor, and I'll set it up for you.`,
        sender: 'assistant',
        timestamp: new Date().toISOString(),
        isWatchContext: true
      };

      setMessages([initialMessage]);
      setSessionTitle('Setting up a Watch');
      setCurrentGreeting('');

      // Clear location state to prevent re-triggering on page refresh
      navigate(location.pathname, { replace: true, state: {} });
    }
  }, [location.state, navigate, location.pathname]);

  // Session title is updated via WebSocket events (session_created, title_update)


  const loadSessionMessages = async (sessionId) => {
    try {
      const response = await apiClient.getSessionMessages(sessionId);

      if (!response || !Array.isArray(response.messages)) {
        setMessages([]);
        return null;
      }

      const processedMessages = response.messages;
      const sessionInfo = response.session;

      // Set session title if available
      if (sessionInfo?.title) {
        setSessionTitle(sessionInfo.title);
      } else {
      }

      // Set session sharing metadata (shared status, owner, Slack sync)
      if (sessionInfo) {
        setSessionMetadata({
          shared: sessionInfo.shared || false,
          created_by: sessionInfo.created_by || null,
          slack_channel_id: sessionInfo.slack_channel_id || null
        });
      }

      // Backend returns consistent ChatMessage schema: message_id, type, content, timestamp, sent_by
      const messagesWithIds = processedMessages.map((message) => {
        return {
          id: message.message_id,
          text: message.content,
          sender: message.type, // "user" or "assistant"
          timestamp: message.timestamp,
          requires_reconnect: message.extra_metadata?.requires_reconnect || false,
          pinned: message.pinned || false,
          sent_by_user_id: message.sent_by_user_id,  // For shared conversation attribution
          sent_by: message.sent_by  // { user_id, display_name }
        };
      });

      setMessages(messagesWithIds);

      // Load thinking events from messages
      const thinkingData = {};
      processedMessages.forEach((message, index) => {
        if (message.thinking_events && Array.isArray(message.thinking_events)) {
          const messageId = messagesWithIds[index].id;
          thinkingData[messageId] = {
            events: message.thinking_events,
            isActive: false, // Always inactive when loading from storage
            tokenUsage: message.token_usage || null // Load token usage if available
          };
        }
      });

      // Update agent thinking state - MERGE with existing state AND buffer to preserve live events
      setAgentThinking(prev => {

        // Merge: Keep existing entries for message IDs not in loaded data
        const merged = { ...prev };
        Object.keys(thinkingData).forEach(messageId => {
          const existing = prev[messageId];
          const loaded = thinkingData[messageId];

          // Check if buffer has events for this message
          const bufferedEvents = thinkingEventBuffer
            .filter(item => item.messageId === messageId)
            .map(item => item.event);

          // Merge all three sources and dedupe by event_id
          const allEvents = [];
          if (existing?.events) allEvents.push(...existing.events);
          if (loaded?.events) allEvents.push(...loaded.events);
          if (bufferedEvents.length > 0) allEvents.push(...bufferedEvents);

          // Dedupe by event_id
          const eventMap = new Map();
          allEvents.forEach(event => {
            if (event.event_id) {
              eventMap.set(event.event_id, event);
            }
          });
          const mergedEvents = Array.from(eventMap.values()).sort((a, b) =>
            a.event_id.localeCompare(b.event_id)
          );

          if (mergedEvents.length > 0) {
            merged[messageId] = {
              events: mergedEvents,
              isActive: existing?.isActive ?? loaded?.isActive ?? false,
              tokenUsage: existing?.tokenUsage ?? loaded?.tokenUsage ?? null
            };
          } else if (loaded) {
            merged[messageId] = loaded;
          }
        });

        return merged;
      });

      return sessionInfo;
    } catch (error) {
      // Gracefully handle by setting empty messages
      setMessages([]);
      return null;
    }
  };

  const switchToSession = async (sessionId) => {
    // Clear current state immediately to prevent flash of old content
    setMessages([]);
    setCurrentGreeting('');
    setIsLoadingSession(true);

    // Update session ID
    updateCurrentSessionId(sessionId);

    // Load messages (URL is already updated by Sidebar navigation)
    const sessionInfo = await loadSessionMessages(sessionId);

    setIsLoadingSession(false);

    // Mark session as read for shared conversations
    if (sessionInfo?.shared) {
      try {
        await apiClient.markSessionRead(sessionId);
      } catch (error) {
        // Non-critical error, don't block the UI
      }
    }
  };

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

  const sendMessageWebSocket = async () => {
    if (!inputMessage.trim() || !chatState.canSend) return;

    // Track first message sent (only once per session)
    if (!hasTrackedFirstMessage.current) {
      trackEvent('first_message_sent');
      hasTrackedFirstMessage.current = true;
    }

    // Mark that we're creating a new session (BEFORE any async operations)
    // This must be synchronous and happen before session_created event can fire
    const isNewSession = !currentSessionIdRef.current;
    if (isNewSession) {
      isCreatingNewSessionRef.current = true;
    }

    // Wait for WebSocket to be connected before sending
    if (connectionState !== 'connected') {
      // Wait up to 5 seconds for connection
      const maxWaitTime = 5000;
      const startTime = Date.now();
      while (connectionState !== 'connected' && Date.now() - startTime < maxWaitTime) {
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      if (connectionState !== 'connected') {
        toast.warning('Connection not ready. Please wait a moment and try again.');
        return;
      }
    }

    const userMessage = {
      id: `user-${Date.now()}-${Math.random().toString(36).substring(2, 11)}`,
      text: inputMessage,
      sender: 'user',
      timestamp: new Date().toISOString()
    };

    setMessages(prev => [...prev, userMessage]);
    let currentInput = inputMessage;
    setInputMessage('');

    // If this is the first message in a chart exploration context, prepend the chart
    // This ensures the backend has the full context when processing the user's question
    if (chartContextRef.current && !currentSessionIdRef.current) {
      currentInput = `Here's a chart I'd like to explore:\n\n${chartContextRef.current}\n\nMy question: ${currentInput}`;
      chartContextRef.current = null; // Clear after first use
    }

    // Start sending state
    chatState.startSending(currentSessionIdRef.current);

    try {
      // Calculate current time for agent awareness of relative date queries
      const timeContext = getTimeContext();

      // Call the WebSocket endpoint - content will be delivered via WebSocket
      // The backend creates the assistant message record immediately and returns the real UUID
      // Pass client_msg_id for deduplication when WebSocket broadcast arrives
      const response = await apiClient.sendMessageWebSocket(
        currentInput,
        currentSessionIdRef.current,
        timeContext,
        skipAiResponse, // Pass skip_ai flag for shared conversation comments
        userMessage.id  // client_msg_id for deduplication
      );

      // If skip_ai was enabled, reset state and return early (no AI response expected)
      if (response.skip_ai) {
        chatState.reset('comment posted');
        setSkipAiResponse(false); // Reset checkbox
        return;
      }

      // Backend returns the REAL message ID
      // Update the optimistic user message with the real ID from backend
      // This prevents duplicates when shared_chat_message broadcasts the same message
      if (response.user_message_id) {
        setMessages(prev => prev.map(msg =>
          msg.id === userMessage.id ? { ...msg, id: response.user_message_id } : msg
        ));
      }

      // Merge thinking_events from API response with buffered WebSocket events
      if (response.thinking_events && response.message_id) {

        // Access current buffer state (avoid stale closure)
        setThinkingEventBuffer(currentBuffer => {

          // Merge API response events + buffered events, dedupe by event_id
          const allEvents = [...response.thinking_events];

          // Add buffered events (if any)
          let bufferedForThisMessage = 0;
          for (const bufferedItem of currentBuffer) {
            if (bufferedItem.messageId === response.message_id) {
              allEvents.push(bufferedItem.event);
              bufferedForThisMessage++;
            }
          }

          // Dedupe and sort by event_id
          const eventMap = new Map();
          for (const event of allEvents) {
            eventMap.set(event.event_id, event);
          }
          const mergedEvents = Array.from(eventMap.values()).sort((a, b) =>
            a.event_id.localeCompare(b.event_id)
          );


          // Set initial thinking state with merged events
          setAgentThinking(prev => {
            const existing = prev[response.message_id];
            const newState = {
              ...prev,
              [response.message_id]: {
                events: mergedEvents,
                isActive: existing?.isActive ?? true, // Preserve isActive if already set by chat_complete
                tokenUsage: response.token_usage || existing?.tokenUsage || null
              }
            };
            return newState;
          });

          // Return filtered buffer (remove events for this message)
          const newBuffer = currentBuffer.filter(item => item.messageId !== response.message_id);
          return newBuffer;
        });
      } else {
      }

      // Update session ID if this was a new chat
      // Just navigate - let useEffect handle updating currentSessionId
      if (response.session_id && !currentSessionIdRef.current) {
        // First message - create new session
        // Only navigate if not already on this session URL (avoid duplicate navigation)
        if (window.location.pathname !== `/chat/${response.session_id}`) {
          navigate(`/chat/${response.session_id}`, { replace: true });
        }
        // Note: Session is added to sidebar via session_created WebSocket event
        // AI title will be updated via WebSocket real-time notification
      } else if (response.session_id && response.session_id !== currentSessionIdRef.current) {
        // Session ID changed unexpectedly - update to match
        // Navigate - useEffect will handle state update
        if (window.location.pathname !== `/chat/${response.session_id}`) {
          navigate(`/chat/${response.session_id}`, { replace: true });
        }
      }

      // The actual streaming content and completion will be handled by WebSocket messages
      // No need to handle streaming here as it's done in the WebSocket onmessage handler

    } catch (error) {

      // Check if error is about AI budget exhaustion
      const errorMessage = error?.response?.data?.detail || error?.message || '';
      const isBudgetExhausted = errorMessage.toLowerCase().includes('ai features are not enabled') ||
                                errorMessage.toLowerCase().includes('budget exhausted') ||
                                errorMessage.toLowerCase().includes('credits exhausted') ||
                                error?.response?.status === 403;

      if (isBudgetExhausted) {
        await refetchCapabilities();
      }

      // Show error message to user
      const userMessage = isBudgetExhausted
        ? 'AI budget exhausted. Please upgrade to continue using AI features.'
        : 'Sorry, I encountered an error. Please try again.';

      setMessages(prev => [...prev, {
        id: `error-${Date.now()}-${Math.random().toString(36).substring(2, 11)}`,
        text: userMessage,
        sender: 'assistant',
        timestamp: new Date().toISOString()
      }]);

      // Transition to error state
      chatState.setErrorState(error.message || 'Failed to send message');
    }
  };

  const handleKeyPress = (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      sendMessageWebSocket();
    }
  };

  const handleCancelRequest = useCallback(() => {
    // Request cancellation through state machine
    const success = chatState.requestCancel();

    if (!success) {
      return;
    }

    if (connectionState !== 'connected') {
      return;
    }

    // Send cancellation message via WebSocket
    const sent = sendWebSocketMessage({
      type: 'cancel_request',
      message_id: chatState.activeMessageId
    });

    if (!sent) {
      // Reset back to streaming since we couldn't send the cancel
      chatState.startStreaming(chatState.activeMessageId);
    }
  }, [chatState, connectionState, sendWebSocketMessage]);

  const updateSessionTitle = async (newTitle) => {
    if (!currentSessionId || !newTitle.trim()) return;

    try {
      await apiClient.updateSessionTitle(currentSessionId, newTitle.trim());
      setSessionTitle(newTitle.trim());
      // Note: Backend sends title_update WebSocket event which Sidebar listens to
      // No need to reload sessions here
    } catch (error) {
    }
  };

  const handleOpenDashboardModal = useCallback((messageContent, messageId) => {
    setDashboardModal({
      isOpen: true,
      messageContent,
      messageId
    });
  }, []);

  // Handler for saving individual chart to dashboard (from chart chrome button)
  const handleSaveChartToDashboard = useCallback((chartMarkdown) => {
    setDashboardModal({
      isOpen: true,
      messageContent: chartMarkdown,
      messageId: null  // No message ID since we're saving just the chart
    });
  }, []);

  // Handler for showing chart info modal (from chart chrome button)
  const handleShowChartInfo = useCallback((spec) => {
    setChartInfoModal({ isOpen: true, spec });
  }, []);

  const handleSaveDashboard = async (mode, titleOrDashboardId, content) => {
    try {
      if (mode === 'new') {
        // Create new dashboard
        const dashboard = await apiClient.createDashboard(titleOrDashboardId, content);
        navigate(`/dashboard/${dashboard.dashboard_id}`);
      } else {
        // Add to existing dashboard
        const dashboard = await apiClient.getDashboard(titleOrDashboardId);
        const updatedContent = dashboard.content + '\n\n---\n\n' + content;
        await apiClient.updateDashboard(titleOrDashboardId, { content: updatedContent });
        navigate(`/dashboard/${titleOrDashboardId}`);
      }
    } catch (error) {
      throw error; // Let the modal handle the error
    }
  };

  const handleTogglePin = useCallback(async (messageId) => {
    try {
      await apiClient.post(`/api/v1/chat/sessions/${currentSessionId}/messages/${messageId}/toggle-pin`);

      // Update local state to reflect the change
      setMessages(prevMessages =>
        prevMessages.map(msg =>
          msg.id === messageId
            ? { ...msg, pinned: !msg.pinned }
            : msg
        )
      );
    } catch (error) {
    }
  }, [apiClient, currentSessionId]);

  const handleMessageUpdate = useCallback(async (messageId, sessionId, updatedMarkdown) => {
    try {
      // Update message content via API
      await apiClient.patch(`/api/v1/chat/sessions/${sessionId}/messages/${messageId}`, {
        content: updatedMarkdown
      });

      // Update local message state to reflect the change
      setMessages(prevMessages =>
        prevMessages.map(msg =>
          msg.id === messageId ? { ...msg, text: updatedMarkdown } : msg
        )
      );
    } catch (error) {
      // Could show toast notification here
    }
  }, []); // Empty dependency array since we use setMessages with function form

  const handleShareSession = useCallback(async () => {
    if (!currentSessionId) return;

    // For Slack DM sessions, clarify that only the Kyomi side is shared
    if (sessionMetadata.slack_channel_id?.startsWith('D')) {
      const confirmed = await confirm({
        title: 'Share with Workspace?',
        message: 'This will share the conversation with your workspace in Kyomi. The original Slack DM will remain private.',
        confirmText: 'Share',
        variant: 'default'
      });
      if (!confirmed) return;
    }

    try {
      await apiClient.shareSession(currentSessionId);
      // Update local state to reflect the change
      setSessionMetadata(prev => ({
        ...prev,
        shared: true
      }));
    } catch (error) {
      toast.error('Failed to share conversation. Please try again.');
    }
  }, [apiClient, currentSessionId, sessionMetadata.slack_channel_id, confirm]);

  const handleUnshareSession = useCallback(async () => {
    if (!currentSessionId) return;

    // Show confirmation dialog
    const confirmed = await confirm({
      title: 'Make Private?',
      message: 'Are you sure you want to make this conversation private? Other workspace members will lose access to it.',
      confirmText: 'Make Private',
      variant: 'default'
    });

    if (!confirmed) return;

    try {
      await apiClient.unshareSession(currentSessionId);
      // Update local state to reflect the change
      setSessionMetadata(prev => ({
        ...prev,
        shared: false
      }));
    } catch (error) {
      toast.error('Failed to unshare conversation. Please try again.');
    }
  }, [apiClient, currentSessionId, confirm]);

  // Show empty state if no datasources are configured
  // While loading, assume datasources exist (optimistic approach)
  // In personal mode without an LLM provider, block chat entirely
  if (isPersonalMode && !llmConfigured) {
    return (
      <div className="flex flex-col items-center justify-center h-full w-full p-8 bg-muted">
        <div className="max-w-md w-full bg-card border border-border rounded-xl p-8 shadow-sm text-center">
          <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
            <svg className="w-8 h-8 text-primary" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M8.625 12a.375.375 0 1 1-.75 0 .375.375 0 0 1 .75 0Zm0 0H8.25m4.125 0a.375.375 0 1 1-.75 0 .375.375 0 0 1 .75 0Zm0 0H12m4.125 0a.375.375 0 1 1-.75 0 .375.375 0 0 1 .75 0Zm0 0h-.375M21 12c0 4.556-4.03 8.25-9 8.25a9.764 9.764 0 0 1-2.555-.337A5.972 5.972 0 0 1 5.41 20.97a5.969 5.969 0 0 1-.474-.065 4.48 4.48 0 0 0 .978-2.025c.09-.457-.133-.901-.467-1.226C3.93 16.178 3 14.189 3 12c0-4.556 4.03-8.25 9-8.25s9 3.694 9 8.25Z" />
            </svg>
          </div>
          <h2 className="text-2xl font-semibold text-foreground mb-3">Chat requires an AI provider</h2>
          <p className="text-muted-foreground mb-6">
            Use Kyomi from Claude Code via MCP, or add your own API key in Settings.
          </p>
          <div className="flex flex-col gap-3">
            <button onClick={() => navigate('/settings/profile')} className="inline-flex items-center justify-center rounded-md bg-primary px-4 py-2.5 text-sm font-medium text-primary-foreground hover:bg-primary/90">
              Open Settings
            </button>
            <button onClick={() => navigate('/setup')} className="inline-flex items-center justify-center rounded-md border border-input bg-background px-4 py-2.5 text-sm font-medium hover:bg-accent hover:text-accent-foreground">
              Learn about MCP
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (!datasourcesLoading && !hasDatasources) {
    return <NoDatasourcesEmptyState context="chat" />;
  }

  return (
        <div className="flex flex-col h-full bg-muted overflow-x-hidden" style={{flexDirection: 'column'}}>
          <div className="flex-1 flex flex-col overflow-hidden">
            {/* Fixed Header with Session Title and Model Selector - Only show when there are messages */}
            {messages.length > 0 && (
              <div className="flex-shrink-0 z-20 bg-card border-b border-border">
                <div className="flex justify-between items-center px-4 md:px-12 py-4 gap-4">
                  {/* Session Title and Sharing Info - Left Side */}
                  <div className="flex items-center gap-2 min-w-0 flex-1 overflow-hidden">
                    {currentSessionId ? (
                      <>
                        <InlineEditableTitle
                          value={sessionTitle}
                          onSave={updateSessionTitle}
                          placeholder="New Chat"
                          className="min-w-0"
                        />
                        {/* Slack Sync Indicator — only for channel threads (C/G prefix), not DMs (D prefix) */}
                        {sessionMetadata.slack_channel_id && sessionMetadata.shared && !sessionMetadata.slack_channel_id.startsWith('D') ? (
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <div>
                                <Badge variant="outline" className="flex-shrink-0 flex items-center gap-1">
                                  <svg className="w-3 h-3" viewBox="0 0 24 24" fill="currentColor">
                                    <path d="M6 15a2 2 0 0 1-2 2a2 2 0 0 1-2-2a2 2 0 0 1 2-2h2v2zm1 0a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-5zm2-8a2 2 0 0 1-2-2a2 2 0 0 1 2-2a2 2 0 0 1 2 2v2H9zm0 1a2 2 0 0 1 2 2a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5zm8 2a2 2 0 0 1 2-2a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-2v-2zm-1 0a2 2 0 0 1-2 2a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5zm-2 8a2 2 0 0 1 2 2a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-2h2zm0-1a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-5z"/>
                                  </svg>
                                  Slack Sync
                                </Badge>
                              </div>
                            </TooltipTrigger>
                            <TooltipContent>
                              Synced with Slack channel thread
                            </TooltipContent>
                          </Tooltip>
                        ) : capabilities.multiUserEnabled ? (
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <div>
                                <Badge variant={sessionMetadata.shared ? "default" : "secondary"} className="flex-shrink-0">
                                  {sessionMetadata.shared ? "Shared" : "Private"}
                                </Badge>
                              </div>
                            </TooltipTrigger>
                            <TooltipContent>
                              {sessionMetadata.shared
                                ? `Owner: ${sessionMetadata.created_by?.display_name || 'Unknown'}`
                                : 'Only you can see this conversation'}
                            </TooltipContent>
                          </Tooltip>
                        ) : null}
                      </>
                    ) : (
                      <div className="text-base font-semibold text-muted-foreground py-1">&nbsp;</div>
                    )}
                  </div>

                  {/* Actions - Right Side */}
                  <div className="flex items-center gap-2">
                    {/* Share Dropdown (owner only, team plan only) */}
                    {currentSessionId && user && sessionMetadata.created_by?.user_id === user.user_id && capabilities.multiUserEnabled && (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                              <button
                                className="flex items-center gap-1.5 px-3 py-1.5 text-sm text-muted-foreground hover:text-foreground hover:bg-accent rounded-lg transition-colors border border-border"
                                aria-label="Share options"
                              >
                                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8.684 13.342C8.886 12.938 9 12.482 9 12c0-.482-.114-.938-.316-1.342m0 2.684a3 3 0 110-2.684m0 2.684l6.632 3.316m-6.632-6l6.632-3.316m0 0a3 3 0 105.367-2.684 3 3 0 00-5.367 2.684zm0 9.316a3 3 0 105.368 2.684 3 3 0 00-5.368-2.684z" />
                                </svg>
                                <span className="hidden sm:inline">Share</span>
                                <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
                                </svg>
                              </button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent align="end">
                              {sessionMetadata.shared ? (
                                sessionMetadata.slack_channel_id && !sessionMetadata.slack_channel_id.startsWith('D') ? (
                                  <Tooltip>
                                    <TooltipTrigger asChild>
                                      <div>
                                        <DropdownMenuItem
                                          disabled
                                          className="opacity-50 cursor-not-allowed"
                                        >
                                          <svg className="w-4 h-4 mr-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
                                          </svg>
                                          Make Private
                                        </DropdownMenuItem>
                                      </div>
                                    </TooltipTrigger>
                                    <TooltipContent>
                                      Slack channel conversations are always shared with your team
                                    </TooltipContent>
                                  </Tooltip>
                                ) : (
                                  <DropdownMenuItem onClick={handleUnshareSession}>
                                    <svg className="w-4 h-4 mr-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
                                    </svg>
                                    Make Private
                                  </DropdownMenuItem>
                                )
                              ) : (
                                <DropdownMenuItem onClick={handleShareSession}>
                                  <svg className="w-4 h-4 mr-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8.684 13.342C8.886 12.938 9 12.482 9 12c0-.482-.114-.938-.316-1.342m0 2.684a3 3 0 110-2.684m0 2.684l6.632 3.316m-6.632-6l6.632-3.316m0 0a3 3 0 105.367-2.684 3 3 0 00-5.367 2.684zm0 9.316a3 3 0 105.368 2.684 3 3 0 00-5.368-2.684z" />
                                  </svg>
                                  Share with Workspace
                                </DropdownMenuItem>
                              )}
                            </DropdownMenuContent>
                          </DropdownMenu>
                        </TooltipTrigger>
                        <TooltipContent>
                          {sessionMetadata.shared
                            ? 'Shared with workspace members'
                            : 'Share this conversation with your workspace'}
                        </TooltipContent>
                      </Tooltip>
                    )}

                    {/* Pinned Messages Filter */}
                    {currentSessionId && messages.some(m => m.pinned) && (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={() => setShowPinnedOnly(!showPinnedOnly)}
                            className={`flex items-center gap-2 px-3 py-1.5 rounded-lg transition-colors text-sm ${
                              showPinnedOnly
                                ? 'bg-accent text-foreground'
                                : 'text-muted-foreground hover:text-foreground hover:bg-accent'
                            }`}
                            aria-label={showPinnedOnly ? 'Show all messages' : 'Show only pinned messages'}
                          >
                            <svg className="w-4 h-4" fill={showPinnedOnly ? 'currentColor' : 'none'} stroke="currentColor" viewBox="0 0 24 24">
                              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                            </svg>
                            <span>{showPinnedOnly ? 'Pinned Only' : 'Pinned'}</span>
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>{showPinnedOnly ? 'Show all messages' : 'Show only pinned messages'}</TooltipContent>
                      </Tooltip>
                    )}
                  </div>
                </div>
              </div>
            )}

            {/* Messages */}
            <div ref={messagesContainerRef} className={`flex-1 overflow-y-auto ${messages.length > 0 ? 'p-4 md:p-6' : ''}`}>
              {messages.length === 0 ? (
                isLoadingSession && currentSessionId ? (
                  // Loading existing session - show loading spinner
                  <div className="h-full flex items-center justify-center">
                    <Spinner size="lg" className="text-muted-foreground" />
                  </div>
                ) : (
                  // New chat - show greeting (vertically centered like login page)
                  <div className="h-full flex items-center justify-center px-4">
                    <div className="text-center w-full max-w-2xl -mt-24">
                      <div className="mb-12">
                        <div className="mb-6">
                          <svg className="w-16 h-16 mx-auto" viewBox="0 0 80 80" xmlns="http://www.w3.org/2000/svg">
                            <g transform="translate(40, 40)">
                              <g fill="#d97706">
                                <polygon points="0,-20 3,-8 0,-5 -3,-8"/>
                                <polygon points="14,-14 8,-3 5,-5 8,-8"/>
                                <polygon points="20,0 8,3 5,0 8,-3"/>
                                <polygon points="14,14 3,8 0,5 3,8"/>
                                <polygon points="0,20 -3,8 0,5 3,8"/>
                                <polygon points="-14,14 -8,3 -5,5 -8,8"/>
                                <polygon points="-20,0 -8,-3 -5,0 -8,3"/>
                                <polygon points="-14,-14 -3,-8 0,-5 -3,-8"/>
                              </g>
                              <circle cx="0" cy="0" r="4" fill="#d97706"/>
                            </g>
                          </svg>
                        </div>
                        <h1 className="text-3xl md:text-4xl font-normal text-foreground mb-8">
                          {currentGreeting}
                        </h1>
                      </div>

                      <div className="mt-8">
                        {creditsExhausted && (
                          <Alert variant="warning" className="mb-4">
                            <AlertDescription className="text-center">
                              {aiDisabledMessage}
                            </AlertDescription>
                          </Alert>
                        )}
                        <div className="relative flex items-center">
                          <textarea
                            ref={textareaRef}
                            value={inputMessage}
                            onChange={(e) => {
                              setInputMessage(e.target.value);
                              e.target.style.height = 'auto';
                              e.target.style.height = Math.min(e.target.scrollHeight, 200) + 'px';
                            }}
                            onKeyPress={handleKeyPress}
                            placeholder={creditsExhausted ? "AI features disabled - upgrade to continue" : "Ask me anything about your data ✨"}
                            className="chat-input w-full px-4 py-3 pr-12 border border-input rounded-xl focus:outline-none focus:ring-2 focus:border-transparent resize-none bg-card shadow-sm text-foreground"
                            rows="1"
                            disabled={!chatState.canSend || creditsExhausted}
                            style={{ "--tw-ring-color": "var(--color-ring)", minHeight: 'auto', maxHeight: '14.29rem' }}
                          />
                          {/* Send button - no cancel button needed on new chat screen */}
                          <button
                            onClick={sendMessageWebSocket}
                            disabled={!inputMessage.trim() || !chatState.canSend || creditsExhausted || connectionState !== 'connected'}
                            className="absolute right-2 top-2 bottom-2 my-auto p-2 text-primary-foreground rounded-lg hover:opacity-90 disabled:bg-muted disabled:cursor-not-allowed transition-opacity flex items-center justify-center bg-primary"
                            aria-label="Send message"
                            title={connectionState !== 'connected' ? 'Waiting for connection...' : 'Send message'}
                          >
                            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
                            </svg>
                          </button>
                        </div>
                      </div>
                    </div>
                  </div>
                )
              ) : (
                <div className="w-full max-w-full space-y-6" style={{display: 'block'}}>
                  {messages
                    .filter(msg => !showPinnedOnly || msg.pinned) // Filter to pinned only if enabled
                    .map((message) => (
                      <ChatMessage
                        key={message.id}
                        message={message}
                        agentThinking={agentThinking}
                        isStreaming={chatState.isStreaming}
                        activeMessageId={chatState.activeMessageId}
                        currentSessionId={currentSessionId}
                        sessionMetadata={sessionMetadata}
                        currentUser={user}
                        onTogglePin={handleTogglePin}
                        onOpenDashboardModal={handleOpenDashboardModal}
                        onMessageUpdate={handleMessageUpdate}
                        onSaveChartToDashboard={handleSaveChartToDashboard}
                        onShowChartInfo={handleShowChartInfo}
                      />
                  ))}
                </div>
              )}
              <div ref={messagesEndRef} />
            </div>
            
            {/* Input Area */}
            {messages.length > 0 && (
              <div className="flex-shrink-0 bg-card border-t border-border p-4 z-10">
                <div className="w-full max-w-full">
                  {creditsExhausted && (
                    <Alert variant="warning" className="mb-4">
                      <AlertDescription className="text-center">
                        {aiDisabledMessage}
                      </AlertDescription>
                    </Alert>
                  )}
                  {connectionState !== 'connected' && (
                    <div className="mb-2 text-sm text-muted-foreground flex items-center gap-2">
                      <div className="w-2 h-2 bg-warning-foreground rounded-full animate-pulse"></div>
                      <span>{connectionState === 'connecting' ? 'Connecting...' : connectionState === 'reconnecting' ? 'Reconnecting...' : 'Disconnected'}</span>
                    </div>
                  )}
                  {/* Skip AI checkbox - show in all existing conversations for adding notes/comments */}
                  {currentSessionId && (
                    <div className="mb-2 flex items-center gap-2">
                      <label className="flex items-center gap-2 text-sm text-muted-foreground cursor-pointer hover:text-foreground transition-colors">
                        <input
                          type="checkbox"
                          checked={skipAiResponse}
                          onChange={(e) => setSkipAiResponse(e.target.checked)}
                          className="h-4 w-4 rounded border-border text-primary focus:ring-ring"
                        />
                        <span>Add as note (skip AI response)</span>
                      </label>
                    </div>
                  )}
                  <div className="relative flex items-center">
                    <textarea
                      value={inputMessage}
                      onChange={(e) => {
                        setInputMessage(e.target.value);
                        e.target.style.height = 'auto';
                        e.target.style.height = Math.min(e.target.scrollHeight, 200) + 'px';
                      }}
                      onKeyPress={handleKeyPress}
                      placeholder={creditsExhausted ? "AI features disabled - upgrade to continue" : "Ask me anything about your data ✨"}
                      className="chat-input w-full px-4 py-3 pr-12 border border-input rounded-xl focus:outline-none focus:ring-2 focus:border-transparent resize-none bg-card shadow-sm overflow-hidden text-foreground"
                      rows="1"
                      disabled={!chatState.canSend || creditsExhausted}
                      style={{ "--tw-ring-color": "var(--color-ring)", minHeight: 'auto', maxHeight: '14.29rem' }}
                    />
                    {(() => {
                      return chatState.showStopButton;
                    })() ? (
                      // Cancel button (shown during processing)
                      <button
                        onClick={handleCancelRequest}
                        disabled={!chatState.canCancel}
                        className="absolute right-2 top-2 bottom-2 my-auto px-3 py-2 bg-destructive hover:bg-destructive/90 disabled:bg-muted disabled:cursor-not-allowed text-destructive-foreground rounded-lg transition-colors flex items-center gap-1.5 z-50"
                        aria-label="Stop generating"
                        title={chatState.canCancel ? "Stop generating" : "Waiting for response..."}
                        style={{ pointerEvents: 'auto' }}
                      >
                        <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                        </svg>
                        <span className="text-sm font-medium">Stop</span>
                      </button>
                    ) : (
                      // Send button (shown when not processing)
                      <button
                        onClick={sendMessageWebSocket}
                        disabled={!inputMessage.trim() || !chatState.canSend || creditsExhausted || connectionState !== 'connected'}
                        className="absolute right-2 top-2 bottom-2 my-auto p-2 text-primary-foreground rounded-lg hover:opacity-90 disabled:bg-muted disabled:cursor-not-allowed transition-opacity flex items-center justify-center bg-primary"
                        aria-label="Send message"
                        title={connectionState !== 'connected' ? 'Waiting for connection...' : 'Send message'}
                      >
                        <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
                        </svg>
                      </button>
                    )}
                  </div>
                </div>
              </div>
            )}
          </div>

        {/* OAuth Reconnection Dialog moved to AppWithOAuthBar - it's now application-wide */}

        {/* Save to Dashboard Modal */}
        <SaveDashboardModal
          isOpen={dashboardModal.isOpen}
          onClose={() => setDashboardModal({ isOpen: false, messageContent: '', messageId: '' })}
          onSave={handleSaveDashboard}
          messageContent={dashboardModal.messageContent}
          apiClient={apiClient}
        />

        {/* Chart Info Modal */}
        <ChartInfoModal
          isOpen={chartInfoModal.isOpen}
          onClose={() => setChartInfoModal({ isOpen: false, spec: null })}
          spec={chartInfoModal.spec}
        />

        {/* Confirm Dialog for share/unshare actions */}
        <ConfirmDialog isOpen={confirmDialogOpen} {...confirmDialogProps} />
        </div>
  );
};

export default Chat;

// Export ChatMessage for reuse in TrialChat
export { ChatMessage };