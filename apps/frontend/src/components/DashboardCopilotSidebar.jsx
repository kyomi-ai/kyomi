// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useRef, useCallback, useEffect } from 'react';
import { useWebSocket } from '../context/WebSocketContext';
import { ChatInterface } from './ChatInterface';
import { XMarkIcon, ChatBubbleLeftRightIcon } from '@heroicons/react/24/outline';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';
import apiClient from '../api/apiClient';

// Hook to detect mobile screen size
function useIsMobile() {
  const [isMobile, setIsMobile] = useState(() =>
    typeof window !== 'undefined' && window.innerWidth < 768
  );

  useEffect(() => {
    const checkMobile = () => setIsMobile(window.innerWidth < 768);
    window.addEventListener('resize', checkMobile);
    return () => window.removeEventListener('resize', checkMobile);
  }, []);

  return isMobile;
}

/**
 * DashboardCopilotSidebar - Conversational AI sidebar for dashboard editing
 *
 * Sessions are stored in DB with session_type='dashboard_copilot' but are
 * ephemeral - they get cleaned up when the sidebar closes.
 * Uses shared ChatInterface component for core chat functionality.
 */
export function DashboardCopilotSidebar({
  isOpen,
  onClose,
  dashboardContent,
  onDashboardUpdate
}) {
  const { subscribe } = useWebSocket();
  const [copilotSessionId, setCopilotSessionId] = useState(null);
  const [contentAtLastMessage, setContentAtLastMessage] = useState(null);
  const copilotSessionIdRef = useRef(null);
  const isMobile = useIsMobile();

  // Resize state (desktop only)
  const [width, setWidth] = useState(384); // Default 384px (w-96)
  const [isResizing, setIsResizing] = useState(false);
  const resizeStartX = useRef(0);
  const resizeStartWidth = useRef(384);

  // Keep ref in sync with state
  useEffect(() => {
    copilotSessionIdRef.current = copilotSessionId;
  }, [copilotSessionId]);

  // Cleanup copilot session when sidebar closes
  const cleanupSession = useCallback(async () => {
    const sessionId = copilotSessionIdRef.current;
    if (sessionId) {
      try {
        await apiClient.delete(`/api/v1/chat/copilot/session/${sessionId}`);
      } catch (error) {
        // Don't fail silently but also don't block - session will be cleaned up by background job
      }
      setCopilotSessionId(null);
      copilotSessionIdRef.current = null;
    }
  }, []);

  // Handle session creation
  const handleSessionCreated = useCallback((sessionId) => {
    setCopilotSessionId(sessionId);
  }, []);

  // Build API payload with dashboard context
  const getApiPayloadExtras = useCallback(() => {
    const contentChanged = dashboardContent !== contentAtLastMessage;
    return {
      context: {
        type: 'dashboard_copilot',
        dashboardContent: contentChanged ? dashboardContent : null
      }
    };
  }, [dashboardContent, contentAtLastMessage]);

  // Handle first thinking event - update content baseline
  const handleFirstThinkingEvent = useCallback(() => {
    if (dashboardContent !== contentAtLastMessage) {
      setContentAtLastMessage(dashboardContent);
    }
  }, [dashboardContent, contentAtLastMessage]);

  // Handle custom WebSocket events (dashboard_update)
  const handleCustomWebSocketEvent = useCallback((subscribeFunc, sessionIdRef) => {
    const unsubscribe = subscribeFunc('dashboard_update', (message) => {
      if (message.data?.context_type !== 'dashboard_copilot') return;
      if (sessionIdRef.current && message.session_id !== sessionIdRef.current) return;

      const newContent = message.data?.content;
      if (newContent && onDashboardUpdate) {
        onDashboardUpdate(newContent);
        setContentAtLastMessage(newContent);
      }
    });
    return unsubscribe;
  }, [onDashboardUpdate]);

  // Handle close with cleanup
  const handleClose = useCallback(async () => {
    await cleanupSession();
    setContentAtLastMessage(null);  // Reset for next open
    onClose();
  }, [cleanupSession, onClose]);

  // Cleanup on unmount (e.g., navigating away from dashboard editor)
  useEffect(() => {
    return () => {
      cleanupSession();
    };
  }, [cleanupSession]);

  // Resize handlers
  const handleResizeStart = useCallback((e) => {
    resizeStartX.current = e.clientX;
    resizeStartWidth.current = width;
    setIsResizing(true);
    e.preventDefault();
  }, [width]);

  useEffect(() => {
    if (!isResizing) return;

    const handleMouseMove = (e) => {
      // Moving left increases width (sidebar is on right)
      const diff = resizeStartX.current - e.clientX;
      const newWidth = resizeStartWidth.current + diff;
      // Clamp between 320px and 600px
      setWidth(Math.max(320, Math.min(newWidth, 600)));
    };

    const handleMouseUp = () => {
      setIsResizing(false);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };

    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);

    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
  }, [isResizing]);

  if (!isOpen) return null;

  // Shared content for both mobile and desktop
  const copilotContent = (
    <div className={`flex flex-col flex-1 min-w-0 ${isMobile ? 'h-full' : ''}`}>
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0">
        <div className="flex items-center gap-2">
          <ChatBubbleLeftRightIcon className="w-5 h-5 text-primary" />
          <span className="font-medium text-foreground">Dashboard Copilot</span>
        </div>
        <button
          onClick={handleClose}
          className="p-1 text-muted-foreground hover:text-foreground rounded-md hover:bg-accent"
          aria-label="Close copilot"
        >
          <XMarkIcon className="w-5 h-5" />
        </button>
      </div>

      {/* Chat Interface */}
      <ChatInterface
        variant="sidebar"
        contextType="dashboard_copilot"
        apiEndpoint="/api/v1/chat/copilot/message"
        apiPayloadExtras={getApiPayloadExtras()}
        sessionId={copilotSessionId}
        onSessionCreated={handleSessionCreated}
        onFirstThinkingEvent={handleFirstThinkingEvent}
        onCustomWebSocketEvent={handleCustomWebSocketEvent}
        placeholder="Ask about your dashboard..."
        emptyStateMessage="Ask me anything about your dashboard!"
        emptyStateSubtext="I can help you improve charts, suggest changes, or make edits directly."
      />
    </div>
  );

  // Mobile: Slide-in panel with backdrop (like main sidebar)
  // Position below the dashboard header so toggle button stays accessible
  // Mobile header (64px) + dashboard header (64px) = 128px, but dashboard header is at 64px
  if (isMobile) {
    return (
      <>
        {/* Backdrop - starts below mobile header bar + dashboard header */}
        <div
          className="fixed top-32 left-0 right-0 bottom-0 bg-black/50 z-40"
          onClick={handleClose}
        />
        {/* Fixed-width panel from right - starts below headers */}
        <div className="fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl">
          {copilotContent}
        </div>
      </>
    );
  }

  // Desktop: Resizable sidebar
  return (
    <div
      className="border-l border-border bg-card flex h-full overflow-hidden"
      style={{ width: `${width}px` }}
    >
      {/* Resize Handle */}
      <Tooltip>
        <TooltipTrigger asChild>
          <div
            className="flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10"
            onMouseDown={handleResizeStart}
            aria-label="Drag to resize"
          >
            <div className="w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded transition-colors" />
          </div>
        </TooltipTrigger>
        <TooltipContent>Drag to resize</TooltipContent>
      </Tooltip>

      {copilotContent}
    </div>
  );
}

export default DashboardCopilotSidebar;
