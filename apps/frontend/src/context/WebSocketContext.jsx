// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { createContext, useContext, useEffect, useRef, useState, useCallback } from 'react';
import { useAuth } from './AuthContext';

/**
 * Centralized WebSocket Context
 *
 * Provides a single, robust WebSocket connection shared across all components.
 * Features:
 * - Single connection per user (no duplicates)
 * - Automatic reconnection with exponential backoff
 * - Event subscription system (components can subscribe to specific message types)
 * - Proper cleanup on unmount
 * - Connection state tracking
 */

const WebSocketContext = createContext(null);

export const useWebSocket = () => {
  const context = useContext(WebSocketContext);
  if (!context) {
    throw new Error('useWebSocket must be used within WebSocketProvider');
  }
  return context;
};

export const WebSocketProvider = ({ children }) => {
  const { user, apiClient } = useAuth();
  const [connectionState, setConnectionState] = useState('disconnected'); // 'disconnected' | 'connecting' | 'connected' | 'reconnecting'
  const wsRef = useRef(null);
  const reconnectTimeoutRef = useRef(null);
  const reconnectAttemptsRef = useRef(0);
  const maxReconnectAttempts = 10;
  const baseReconnectDelay = 1000; // 1 second
  const subscribersRef = useRef(new Map()); // Map of message_type -> Set of callback functions
  const isIntentionalCloseRef = useRef(false);

  // Subscribe to specific message types
  const subscribe = useCallback((messageType, callback) => {

    if (!subscribersRef.current.has(messageType)) {
      subscribersRef.current.set(messageType, new Set());
    }
    subscribersRef.current.get(messageType).add(callback);

    // Return unsubscribe function
    return () => {
      const callbacks = subscribersRef.current.get(messageType);
      if (callbacks) {
        callbacks.delete(callback);
        if (callbacks.size === 0) {
          subscribersRef.current.delete(messageType);
        }
      }
    };
  }, []);

  // Send message through WebSocket
  const send = useCallback((message) => {
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(message));
      return true;
    } else {
      return false;
    }
  }, []);

  // Connect to WebSocket
  const connect = useCallback(async () => {
    if (!user?.user_id || !user?.workspace_id) {
      return;
    }

    // Close existing connection if it exists (handles HMR and reconnection)
    if (wsRef.current) {
      if (wsRef.current.readyState === WebSocket.OPEN) {
        return;
      }
      // Clean up stale connection
      wsRef.current.onclose = null; // Prevent reconnection logic from firing
      wsRef.current.close();
      wsRef.current = null;
    }

    setConnectionState('connecting');

    try {
      // Get WebSocket authentication token
      const tokenResponse = await apiClient.get('/api/v1/auth/websocket-token');
      const wsToken = tokenResponse.data.token;

      const userIdValue = user.user_id || user.id;
      const workspaceUserId = `${user.workspace_id}_${userIdValue}`;

      // Use same domain (goes through nginx proxy)
      const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
      const host = window.location.host; // Includes port if present
      const wsUrl = `${protocol}//${host}/ws/${workspaceUserId}?token=${wsToken}`;

      const ws = new WebSocket(wsUrl);
      wsRef.current = ws;

      ws.onopen = () => {
        setConnectionState('connected');
        reconnectAttemptsRef.current = 0; // Reset reconnect attempts on successful connection
      };

      ws.onmessage = (event) => {
        try {
          const message = JSON.parse(event.data);

          // Duplicate detection
          const messageKey = `${message.type}_${message.session_id}_${message.message_id}_${JSON.stringify(message.data)}`;
          if (!window.receivedWebSocketMessages) {
            window.receivedWebSocketMessages = new Set();
          }
          if (window.receivedWebSocketMessages.has(messageKey)) {
            return; // Skip duplicate
          }
          window.receivedWebSocketMessages.add(messageKey);


          // Notify all subscribers for this message type
          const callbacks = subscribersRef.current.get(message.type);
          if (callbacks && callbacks.size > 0) {
            callbacks.forEach(callback => {
              try {
                callback(message);
              } catch (error) {
              }
            });
          } else {
          }
        } catch (error) {
        }
      };

      ws.onerror = (error) => {
        setConnectionState('disconnected');
      };

      ws.onclose = (event) => {
        setConnectionState('disconnected');
        wsRef.current = null;

        // Only attempt reconnection if not intentionally closed
        if (!isIntentionalCloseRef.current && reconnectAttemptsRef.current < maxReconnectAttempts) {
          const delay = Math.min(
            baseReconnectDelay * Math.pow(2, reconnectAttemptsRef.current),
            30000 // Max 30 seconds
          );

          reconnectAttemptsRef.current++;
          setConnectionState('reconnecting');

          reconnectTimeoutRef.current = setTimeout(() => {
            connect();
          }, delay);
        } else if (reconnectAttemptsRef.current >= maxReconnectAttempts) {
        }
      };

    } catch (error) {
      setConnectionState('disconnected');

      // Retry connection on initial failure (e.g., 502 during backend restart)
      if (!isIntentionalCloseRef.current && reconnectAttemptsRef.current < maxReconnectAttempts) {
        const delay = Math.min(
          baseReconnectDelay * Math.pow(2, reconnectAttemptsRef.current),
          30000 // Max 30 seconds
        );

        reconnectAttemptsRef.current++;
        setConnectionState('reconnecting');

        reconnectTimeoutRef.current = setTimeout(() => {
          connect();
        }, delay);
      }
    }
  }, [user, apiClient]);

  // Disconnect WebSocket
  const disconnect = useCallback(() => {
    isIntentionalCloseRef.current = true;

    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = null;
    }

    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }

    setConnectionState('disconnected');
  }, []);

  // Auto-connect when user is available
  useEffect(() => {
    if (user?.user_id && user?.workspace_id) {
      isIntentionalCloseRef.current = false; // Reset flag before connecting
      connect();
    }

    // Cleanup on unmount or user change
    return () => {
      // Mark as intentional close to prevent reconnection during cleanup
      isIntentionalCloseRef.current = true;

      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current);
        reconnectTimeoutRef.current = null;
      }

      if (wsRef.current) {
        // CRITICAL: Null out onclose before closing to prevent the old WS's
        // onclose handler from firing after the new effect resets
        // isIntentionalCloseRef to false — which would trigger a spurious reconnect
        // and create a duplicate connection.
        wsRef.current.onclose = null;
        wsRef.current.onerror = null;
        wsRef.current.close();
        wsRef.current = null;
      }

      setConnectionState('disconnected');
    };
  }, [user?.user_id, user?.workspace_id, connect]);

  const value = {
    connectionState,
    subscribe,
    send,
    connect,
    disconnect,
  };

  return (
    <WebSocketContext.Provider value={value}>
      {children}
    </WebSocketContext.Provider>
  );
};
