// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useRef, useEffect, useCallback } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { Button } from '@/components/ui/button';
import { Spinner } from './ui/spinner';
import { ChatMessage } from '../pages/Chat';
import ChartInfoModal from './ChartInfoModal';
import { processThinkingEvent } from '../hooks/useAgentThinking';
import { sendTrialMessage, ensureTrialSession, getSessionToken, getTrialAccessToken, clearSessionToken } from '../api/trialApi';
import { trackEvent } from '../utils/analytics';
import { API_CONFIG } from '../config/api.js';

// Suggested questions for the trial
const SUGGESTED_QUESTIONS = [
  "What was our MRR trend last quarter?",
  "Show me the top 10 customers by revenue",
  "What's our churn rate by plan type?",
  "Which landing pages have the best conversion?",
];

// Marketing site URL (localhost in dev, production otherwise)
const MARKETING_URL = window.location.hostname === 'localhost'
  ? 'http://localhost:5175'
  : 'https://kyomi.ai';

// SignupPromptModal component - shown when trial limit is reached
function SignupPromptModal({ isOpen }) {
  const navigate = useNavigate();

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      {/* Backdrop - not dismissible */}
      <div className="absolute inset-0 bg-black/50" />

      {/* Modal */}
      <div className="relative z-10 w-full max-w-md p-6 mx-4 bg-card rounded-xl shadow-xl border border-border">
        <h2 className="text-xl font-semibold text-foreground mb-2">
          You've reached your trial limit
        </h2>
        <p className="text-muted-foreground mb-6">
          Sign up to get unlimited access to Kyomi and connect your own data.
        </p>

        <Button
          onClick={() => {
            trackEvent('trial_signup_click', { from_modal: true });
            navigate('/login');
          }}
          className="w-full"
        >
          Sign Up Free
        </Button>
      </div>
    </div>
  );
}

// Main TrialChat component - reuses ChatMessage from the main Chat for full ChartML/thinking tracker support
export default function TrialChat() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [messages, setMessages] = useState([]);
  const [conversationHistory, setConversationHistory] = useState([]);
  const [inputValue, setInputValue] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [isInitializing, setIsInitializing] = useState(true);  // Session initialization
  const [queryCount, setQueryCount] = useState(0);
  const [queriesRemaining, setQueriesRemaining] = useState(5);
  const [showSignupModal, setShowSignupModal] = useState(false);
  const [error, setError] = useState(null);
  const [agentThinking, setAgentThinking] = useState({});
  const [chartInfoModal, setChartInfoModal] = useState({ isOpen: false, spec: null });
  const messagesEndRef = useRef(null);
  const inputRef = useRef(null);
  const messageIdCounter = useRef(0);
  const hasAutoSubmitted = useRef(false);
  const wsRef = useRef(null);
  const currentMessageIdRef = useRef(null);  // Track current message for WebSocket events

  // Initialize trial session on mount
  useEffect(() => {
    async function initSession() {
      try {
        const session = await ensureTrialSession();
        if (session.queries_remaining !== undefined) {
          setQueriesRemaining(session.queries_remaining);
          setQueryCount(5 - session.queries_remaining);
        }
        setIsInitializing(false);
      } catch (err) {
        console.error('Failed to create trial session:', err);
        setError(err.message || 'Failed to initialize trial session');
        setIsInitializing(false);
        // If rate limited, show signup modal
        if (err.message?.includes('limit')) {
          setShowSignupModal(true);
        }
      }
    }
    initSession();
  }, []);

  // Generate unique message ID
  const generateMessageId = useCallback(() => {
    messageIdCounter.current += 1;
    return `trial-msg-${Date.now()}-${messageIdCounter.current}`;
  }, []);

  // Scroll to bottom when messages change
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  // Focus input on mount
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // WebSocket connection for real-time thinking events
  const connectWebSocket = useCallback((sessionId, accessToken, messageId) => {
    // Close existing connection if any
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }

    // Build WebSocket URL
    // Use window.location.host when baseURL is empty (dev proxy mode)
    const wsProtocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    let wsHost = window.location.host;  // Default to current host
    if (API_CONFIG.baseURL && API_CONFIG.baseURL.startsWith('http')) {
      // Production mode with explicit baseURL
      wsHost = API_CONFIG.baseURL.replace(/^https?:\/\//, '').replace(/\/$/, '');
    }
    const wsUrl = `${wsProtocol}//${wsHost}/ws/trial/${sessionId}?token=${encodeURIComponent(accessToken)}`;

    console.log('[TrialWS] Connecting to:', wsUrl);
    const ws = new WebSocket(wsUrl);

    ws.onopen = () => {
      console.log('[TrialWS] Connected');
    };

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);
        console.log('[TrialWS] Message:', data.type);

        // Handle thinking events
        if (data.type === 'agent_thinking' && data.data?.event) {
          const thinkingEvent = data.data.event;
          const msgId = currentMessageIdRef.current || messageId;

          setAgentThinking(prev => {
            const existing = prev[msgId] || { events: [], isActive: true, tokenUsage: null };
            // Filter out the 'init' placeholder when real events arrive
            const filteredEvents = existing.events.filter(e => e.event_id !== 'init');
            const newEvents = processThinkingEvent(filteredEvents, thinkingEvent);
            return {
              ...prev,
              [msgId]: {
                ...existing,
                events: newEvents,
                isActive: thinkingEvent.event_type !== 'agent_complete',
              }
            };
          });
        }

        // Handle token usage updates
        if (data.type === 'token_usage_update' && data.data?.token_usage) {
          const msgId = currentMessageIdRef.current || messageId;
          setAgentThinking(prev => {
            const existing = prev[msgId] || { events: [], isActive: false, tokenUsage: null };
            return {
              ...prev,
              [msgId]: {
                ...existing,
                tokenUsage: data.data.token_usage,
              }
            };
          });
        }
      } catch (e) {
        console.error('[TrialWS] Parse error:', e);
      }
    };

    ws.onerror = (error) => {
      console.error('[TrialWS] Error:', error);
    };

    ws.onclose = (event) => {
      console.log('[TrialWS] Closed:', event.code, event.reason);
    };

    wsRef.current = ws;
    return ws;
  }, []);

  // Cleanup WebSocket on unmount
  useEffect(() => {
    return () => {
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
    };
  }, []);

  // Handler for showing chart info modal
  const handleShowChartInfo = useCallback((spec) => {
    setChartInfoModal({ isOpen: true, spec });
  }, []);

  const handleSubmit = useCallback(async (e, suggestedQuestion = null) => {
    e?.preventDefault();
    const messageText = suggestedQuestion || inputValue.trim();

    if (!messageText || isLoading) return;

    setError(null);
    setInputValue('');
    setIsLoading(true);

    // Generate local user message ID (backend doesn't need it)
    const userMsgId = generateMessageId();
    // Temporary placeholder ID for assistant - will be replaced with backend message_id
    const tempAssistantMsgId = generateMessageId();

    // Add user message to UI (using ChatMessage format)
    const userMessage = {
      id: userMsgId,
      text: messageText,
      sender: 'user',
      timestamp: new Date().toISOString(),
    };
    setMessages(prev => [...prev, userMessage]);

    // Add placeholder assistant message (for thinking tracker)
    const assistantPlaceholder = {
      id: tempAssistantMsgId,
      text: '',
      sender: 'assistant',
      timestamp: new Date().toISOString(),
      isStreaming: true,
    };
    setMessages(prev => [...prev, assistantPlaceholder]);

    // Set initial thinking state with placeholder
    setAgentThinking(prev => ({
      ...prev,
      [tempAssistantMsgId]: {
        events: [{ event_id: 'init', event_type: 'agent_start', title: 'Starting analysis', description: 'Analyzing your question...' }],
        isActive: true,
        tokenUsage: null,
      }
    }));

    // Store temp message ID for WebSocket handler
    currentMessageIdRef.current = tempAssistantMsgId;

    try {
      trackEvent('trial_message_sent', { query_number: queryCount + 1 });

      // Connect WebSocket for real-time thinking events (if we have tokens from previous message)
      const sessionToken = getSessionToken();
      const accessToken = getTrialAccessToken();
      if (sessionToken && accessToken) {
        const sessionId = `trial_${sessionToken}`;
        connectWebSocket(sessionId, accessToken, tempAssistantMsgId);
        // Give WebSocket a moment to connect
        await new Promise(resolve => setTimeout(resolve, 100));
      }

      const response = await sendTrialMessage(
        messageText,
        conversationHistory
      );

      // Update state from response
      setQueryCount(response.query_count);
      setQueriesRemaining(response.queries_remaining);

      // Get the real message_id from the response
      const realMessageId = response.message_id;

      // Update message ID ref for any remaining WebSocket events
      currentMessageIdRef.current = realMessageId;

      // Update assistant message with real response and real message_id
      setMessages(prev => prev.map(msg =>
        msg.id === tempAssistantMsgId
          ? { ...msg, id: realMessageId, text: response.response, isStreaming: false }
          : msg
      ));

      // Update agentThinking: merge WebSocket events (real-time) with HTTP response events
      // WebSocket events are under tempAssistantMsgId, HTTP events are the fallback
      setAgentThinking(prev => {
        // Filter out the 'init' placeholder from WebSocket events
        const wsEvents = (prev[tempAssistantMsgId]?.events || []).filter(e => e.event_id !== 'init');
        const wsTokenUsage = prev[tempAssistantMsgId]?.tokenUsage;

        // If WebSocket gave us real events (not just placeholder), use those
        // Otherwise fall back to HTTP response events
        let finalEvents = wsEvents;
        if (wsEvents.length === 0 && response.thinking_events?.length > 0) {
          // WebSocket didn't work, use HTTP events
          finalEvents = [];
          for (const event of response.thinking_events) {
            finalEvents = processThinkingEvent(finalEvents, event);
          }
        }

        const newState = { ...prev };
        delete newState[tempAssistantMsgId];
        newState[realMessageId] = {
          events: finalEvents,
          isActive: false,
          tokenUsage: wsTokenUsage || null,
        };
        return newState;
      });

      // Close WebSocket after response (events are complete)
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }

      // Update conversation history (keep last 10 for API)
      setConversationHistory(prev => {
        const newHistory = [
          ...prev,
          { role: 'user', content: messageText },
          { role: 'assistant', content: response.response }
        ];
        return newHistory.slice(-10);
      });


    } catch (err) {
      if (err.message?.includes('limit')) {
        // Query limit reached - show signup modal
        setShowSignupModal(true);
        // Remove both user and assistant placeholder
        setMessages(prev => prev.filter(msg => msg.id !== userMsgId && msg.id !== tempAssistantMsgId));
        trackEvent('trial_limit_reached');
      } else {
        setError(err.message || 'Something went wrong. Please try again.');
        // Remove both messages on error
        setMessages(prev => prev.filter(msg => msg.id !== userMsgId && msg.id !== tempAssistantMsgId));
      }
      // Clear thinking state on error
      setAgentThinking(prev => {
        const newState = { ...prev };
        delete newState[tempAssistantMsgId];
        return newState;
      });
    } finally {
      setIsLoading(false);
      currentMessageIdRef.current = null;
    }
  }, [inputValue, isLoading, conversationHistory, queryCount, generateMessageId, connectWebSocket]);

  const handleSuggestedQuestion = useCallback((question) => {
    handleSubmit(null, question);
  }, [handleSubmit]);

  const handleReset = useCallback(async () => {
    // Close WebSocket if open
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }
    clearSessionToken();
    setMessages([]);
    setConversationHistory([]);
    setAgentThinking({});
    setQueryCount(0);
    setQueriesRemaining(5);
    setError(null);
    trackEvent('trial_reset');

    // Note: Session will be re-created on next message send via ensureTrialSession
    // But since we're same IP same day, we'll get the same session token
  }, []);

  // Auto-submit question from URL query param (from marketing site)
  useEffect(() => {
    const question = searchParams.get('q');
    // Wait for session initialization before auto-submitting
    if (question && !hasAutoSubmitted.current && !isLoading && !isInitializing) {
      hasAutoSubmitted.current = true;
      // Clear the query param to avoid re-submission on refresh
      setSearchParams({}, { replace: true });
      // Track that user came from marketing site
      trackEvent('trial_from_marketing', { question_length: question.length });
      // Submit the question
      handleSubmit(null, question);
    }
  }, [searchParams, setSearchParams, isLoading, isInitializing, handleSubmit]);

  // Dummy session metadata for ChatMessage
  const sessionMetadata = { shared: false, created_by: null, slack_channel_id: null };

  return (
    <div className="flex flex-col h-full bg-background">
      {/* Header */}
      <header className="flex-shrink-0 border-b border-border bg-background px-4 py-3">
        <div className="max-w-4xl mx-auto flex items-center justify-between">
          <a
            href={MARKETING_URL}
            className="flex items-center gap-3 hover:opacity-80 transition-opacity"
          >
            <img
              src="/kyomi_full_logo.svg"
              alt="Kyomi"
              className="h-10 dark:hidden"
            />
            <img
              src="/kyomi_full_logo_white.svg"
              alt="Kyomi"
              className="h-10 hidden dark:block"
            />
            <span className="text-muted-foreground text-sm hidden sm:inline">
              · Sample Data Explorer
            </span>
          </a>

          <div className="flex items-center gap-4">
            <span className="text-sm text-muted-foreground hidden sm:inline">
              {queriesRemaining} queries remaining
            </span>
            <Button
              variant="default"
              size="sm"
              onClick={() => {
                trackEvent('trial_signup_click', { from_header: true });
                navigate('/login');
              }}
            >
              Sign Up Free
            </Button>
          </div>
        </div>
      </header>

      {/* Messages area */}
      <div className="flex-1 overflow-y-auto">
        <div className="max-w-4xl mx-auto px-4 py-6">
          {messages.length === 0 ? (
            // Welcome state
            <div className="space-y-6">
              <div className="text-center">
                <h2 className="text-2xl font-semibold text-foreground mb-2">
                  Welcome to Kyomi
                </h2>
                <p className="text-muted-foreground max-w-md mx-auto">
                  Ask questions about the sample SaaS dataset below.
                  We've loaded 18 months of data from a fictional company called Acme Analytics.
                </p>
              </div>

              {/* Sample data info */}
              <div className="bg-card border border-border rounded-xl p-4">
                <h3 className="font-medium text-foreground mb-3">Available Data</h3>
                <div className="grid grid-cols-2 gap-3 text-sm">
                  <div className="flex items-start gap-2">
                    <div className="w-2 h-2 rounded-full bg-primary mt-1.5 flex-shrink-0" />
                    <div>
                      <span className="font-medium text-foreground">Subscriptions</span>
                      <p className="text-muted-foreground text-xs">MRR, plans, churn data</p>
                    </div>
                  </div>
                  <div className="flex items-start gap-2">
                    <div className="w-2 h-2 rounded-full bg-primary mt-1.5 flex-shrink-0" />
                    <div>
                      <span className="font-medium text-foreground">Users</span>
                      <p className="text-muted-foreground text-xs">Signups, roles, activity</p>
                    </div>
                  </div>
                  <div className="flex items-start gap-2">
                    <div className="w-2 h-2 rounded-full bg-primary mt-1.5 flex-shrink-0" />
                    <div>
                      <span className="font-medium text-foreground">Events</span>
                      <p className="text-muted-foreground text-xs">Feature usage, 50k+ events</p>
                    </div>
                  </div>
                  <div className="flex items-start gap-2">
                    <div className="w-2 h-2 rounded-full bg-primary mt-1.5 flex-shrink-0" />
                    <div>
                      <span className="font-medium text-foreground">Website Sessions</span>
                      <p className="text-muted-foreground text-xs">Funnel, conversions</p>
                    </div>
                  </div>
                </div>
              </div>

              {/* Suggested questions */}
              <div>
                <p className="text-sm text-muted-foreground mb-3 text-center">Try one of these:</p>
                <div className="flex flex-wrap gap-2 justify-center">
                  {SUGGESTED_QUESTIONS.map((question, idx) => (
                    <button
                      key={idx}
                      onClick={() => handleSuggestedQuestion(question)}
                      disabled={isLoading || isInitializing}
                      className="px-3 py-2 text-sm bg-card border border-border rounded-lg hover:border-primary hover:text-primary transition-colors disabled:opacity-50"
                    >
                      {question}
                    </button>
                  ))}
                </div>
              </div>
            </div>
          ) : (
            // Messages list using ChatMessage component for full ChartML support
            <div className="w-full space-y-6">
              {messages.map((message) => (
                <ChatMessage
                  key={message.id}
                  message={message}
                  agentThinking={agentThinking}
                  isStreaming={isLoading && message.isStreaming}
                  activeMessageId={isLoading ? message.id : null}
                  currentSessionId="trial-session"
                  sessionMetadata={sessionMetadata}
                  currentUser={null}
                  onTogglePin={null}
                  onOpenDashboardModal={null}
                  onMessageUpdate={null}
                  onSaveChartToDashboard={null}
                  onShowChartInfo={handleShowChartInfo}
                  isTrialMode={true}
                />
              ))}

              <div ref={messagesEndRef} />
            </div>
          )}
        </div>
      </div>

      {/* Error display */}
      {error && (
        <div className="flex-shrink-0 px-4 pb-2">
          <div className="max-w-4xl mx-auto">
            <div className="bg-error text-error-foreground border border-error-border rounded-lg px-4 py-3 text-sm">
              {error}
            </div>
          </div>
        </div>
      )}

      {/* Input area */}
      <footer className="flex-shrink-0 border-t border-border bg-background px-4 py-3">
        <div className="max-w-4xl mx-auto">
          <form onSubmit={handleSubmit} className="flex gap-2">
            <input
              ref={inputRef}
              type="text"
              value={inputValue}
              onChange={(e) => setInputValue(e.target.value)}
              placeholder={isInitializing ? "Initializing trial session..." : "Ask a question about the data..."}
              disabled={isLoading || isInitializing}
              className="flex-1 px-4 py-2 border border-input rounded-lg focus:outline-none focus:ring-2 focus:ring-ring focus:border-transparent disabled:opacity-50 bg-background text-foreground"
            />
            <Button type="submit" disabled={isLoading || isInitializing || !inputValue.trim()}>
              {isLoading ? <Spinner size="sm" /> : 'Send'}
            </Button>
          </form>

          {queryCount > 0 && (
            <div className="flex justify-between items-center mt-2 text-xs text-muted-foreground">
              <button
                onClick={handleReset}
                className="hover:text-foreground transition-colors"
              >
                Reset conversation
              </button>
              <span>{queryCount}/5 queries used</span>
            </div>
          )}
        </div>
      </footer>

      {/* Signup prompt modal */}
      <SignupPromptModal isOpen={showSignupModal} />

      {/* Chart Info Modal */}
      <ChartInfoModal
        isOpen={chartInfoModal.isOpen}
        onClose={() => setChartInfoModal({ isOpen: false, spec: null })}
        spec={chartInfoModal.spec}
      />
    </div>
  );
}
