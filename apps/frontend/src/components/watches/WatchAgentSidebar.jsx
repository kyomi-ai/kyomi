// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useRef, useCallback, useEffect } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { ChatInterface } from '../ChatInterface';
import { Tooltip, TooltipTrigger, TooltipContent } from '../ui/tooltip';
import apiClient from '../../api/apiClient';
import { XMarkIcon, EyeIcon } from '@heroicons/react/24/outline';

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
 * WatchAgentSidebar - AI-powered sidebar for creating and editing watches
 *
 * Uses the shared ChatInterface with watch_copilot context type.
 * The agent outputs structured ```json:watch-response blocks with
 * {message, watch} format. MarkdownRenderer renders the message and
 * optional watch preview card.
 *
 * @param {boolean} isOpen - Whether the sidebar is open
 * @param {Function} onClose - Close callback
 * @param {Object} editingWatch - Existing watch to edit (null for create mode)
 * @param {Function} onWatchCreated - Called when a watch is created/updated
 */
export function WatchAgentSidebar({
  isOpen,
  onClose,
  editingWatch = null,
  onWatchCreated,
}) {
  const queryClient = useQueryClient();
  const [sessionId, setSessionId] = useState(null);
  const sessionIdRef = useRef(null);
  const isMobile = useIsMobile();

  // Track accepted watch card IDs (stable across re-renders)
  const [acceptedCardIds, setAcceptedCardIds] = useState(new Set());

  // Track newly created watch from approved cards (so agent knows the watch_id)
  const [agentEditingWatch, setAgentEditingWatch] = useState(null);

  // Resize state (desktop only)
  const [width, setWidth] = useState(420);
  const [isResizing, setIsResizing] = useState(false);
  const resizeStartX = useRef(0);
  const resizeStartWidth = useRef(420);

  const mode = editingWatch ? 'update' : 'create';

  // Keep ref in sync with state
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  // Cleanup session (defined before effects that use it)
  const cleanupSession = useCallback(async () => {
    const sid = sessionIdRef.current;
    if (sid) {
      try {
        await apiClient.delete(`/api/v1/chat/copilot/session/${sid}`);
      } catch (error) {
      }
      setSessionId(null);
      sessionIdRef.current = null;
    }
    setAcceptedCardIds(new Set());
  }, []);

  // Reset session when switching between watches or create/edit mode
  const editingWatchId = editingWatch?.watch_id;
  const prevEditingWatchId = useRef(editingWatchId);
  useEffect(() => {
    // Only cleanup when actually CHANGING watches, not on initial mount
    if (prevEditingWatchId.current !== editingWatchId) {
      cleanupSession();
    }
    prevEditingWatchId.current = editingWatchId;
  }, [editingWatchId, cleanupSession]);

  // Handle session creation
  const handleSessionCreated = useCallback((newSessionId) => {
    setSessionId(newSessionId);
  }, []);

  // Build API payload with watch context
  const getApiPayloadExtras = useCallback(() => {
    // Get user's timezone from browser
    const userTimezone = Intl.DateTimeFormat().resolvedOptions().timeZone;

    const extras = {
      context: {
        type: 'watch_copilot',
        timezone: userTimezone,
      }
    };

    // For edit mode, include the existing watch config
    // Prefer agentEditingWatch (newly created watch) over editingWatch prop (parent-provided watch)
    const watchToSend = agentEditingWatch || editingWatch;
    if (watchToSend) {
      extras.context.watchConfig = {
        watch_id: watchToSend.watch_id,
        name: watchToSend.name,
        prompt: watchToSend.prompt,
        schedule: watchToSend.schedule,
        enabled: watchToSend.enabled,
      };
    }

    return extras;
  }, [editingWatch, agentEditingWatch]);

  // Handle watch approved via inline preview card
  const handleWatchApproved = useCallback((watchData, cardId) => {

    // Track this card as accepted
    if (cardId) {
      setAcceptedCardIds(prev => new Set([...prev, cardId]));
    }

    // If this is a newly created watch (has watch_id), update editingWatch so the agent knows about it
    // This allows the agent to reference this watch in future updates
    if (watchData?.watch_id) {
      setAgentEditingWatch({
        watch_id: watchData.watch_id,
        name: watchData.name,
        prompt: watchData.prompt,
        schedule: watchData.schedule,
        enabled: watchData.enabled,
      });
    }

    // Refresh watches list
    queryClient.invalidateQueries(['watches']);

    // Notify parent
    onWatchCreated?.();
  }, [queryClient, onWatchCreated]);

  // Handle close with cleanup
  const handleClose = useCallback(async () => {
    await cleanupSession();
    onClose();
  }, [cleanupSession, onClose]);

  // Cleanup on unmount
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
      const diff = resizeStartX.current - e.clientX;
      const newWidth = resizeStartWidth.current + diff;
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

  // Build empty state message based on mode
  const emptyStateMessage = mode === 'create'
    ? "What would you like to monitor?"
    : `Editing: ${editingWatch?.name}`;

  const emptyStateSubtext = mode === 'create'
    ? "Describe what data to watch and when to alert you."
    : "Tell me what you'd like to change.";

  const placeholder = mode === 'create'
    ? "Alert me when daily revenue drops more than 10%..."
    : "Make it run hourly instead...";

  // Shared content
  const sidebarContent = (
    <div className={`flex flex-col flex-1 min-w-0 ${isMobile ? 'h-full' : ''}`}>
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0">
        <div className="flex items-center gap-2">
          <EyeIcon className="w-5 h-5 text-primary" />
          <span className="font-medium text-foreground">
            {mode === 'create' ? 'Create Watch' : 'Edit Watch'}
          </span>
        </div>
        <button
          onClick={handleClose}
          className="p-1 text-muted-foreground hover:text-foreground rounded-md hover:bg-accent"
          aria-label="Close"
        >
          <XMarkIcon className="w-5 h-5" />
        </button>
      </div>

      {/* Chat Interface */}
      <ChatInterface
        variant="sidebar"
        contextType="watch_copilot"
        apiEndpoint="/api/v1/chat/copilot/message"
        apiPayloadExtras={getApiPayloadExtras()}
        sessionId={sessionId}
        onSessionCreated={handleSessionCreated}
        placeholder={placeholder}
        emptyStateMessage={emptyStateMessage}
        emptyStateSubtext={emptyStateSubtext}
        onWatchApproved={handleWatchApproved}
        acceptedCardIds={acceptedCardIds}
      />
    </div>
  );

  // Mobile: Slide-in panel with backdrop
  if (isMobile) {
    return (
      <>
        <div
          className="fixed top-16 left-0 right-0 bottom-0 bg-black/50 z-40"
          onClick={handleClose}
        />
        <div className="fixed top-16 right-0 bottom-0 w-full max-w-[92vw] z-50 bg-background flex flex-col shadow-xl">
          {sidebarContent}
        </div>
      </>
    );
  }

  // Desktop: Resizable sidebar
  return (
    <div
      className="border-l border-border bg-background flex h-full overflow-hidden"
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
            <div className="w-1 h-12 bg-border hover:bg-muted-foreground rounded transition-colors" />
          </div>
        </TooltipTrigger>
        <TooltipContent>Drag to resize</TooltipContent>
      </Tooltip>

      {sidebarContent}
    </div>
  );
}

export default WatchAgentSidebar;
