// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, createContext, useContext } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { useAuth } from '../context/AuthContext';
import { useCapabilities } from '../context/CapabilitiesContext';
import { useSystemConfig } from '../context/SystemConfigContext';
import { useWebSocket } from '../context/WebSocketContext';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';
import { Badge } from './ui/badge';
import WorkspaceSwitcher from './WorkspaceSwitcher';
import ConfirmDialog from './ConfirmDialog';
import useConfirm from '../hooks/useConfirm';
import { toast } from '../lib/toast';
import FeedbackModal from './feedback/FeedbackModal';

// Create context for sidebar state
const SidebarContext = createContext({ isSidebarCollapsed: false });

export const useSidebar = () => useContext(SidebarContext);

export const SidebarProvider = ({ children }) => {
  const [isSidebarCollapsed, setIsSidebarCollapsed] = useState(() => {
    // Initialize collapsed state based on window size
    if (typeof window === 'undefined') return false;

    // Default: collapse on mobile, expand on desktop
    return window.innerWidth < 768;
  });

  return (
    <SidebarContext.Provider value={{ isSidebarCollapsed, setIsSidebarCollapsed }}>
      {children}
    </SidebarContext.Provider>
  );
};

const Sidebar = () => {
  const { isSidebarCollapsed, setIsSidebarCollapsed } = useSidebar();
  const navigate = useNavigate();
  const location = useLocation();
  const { user, logout, apiClient } = useAuth();
  const capabilities = useCapabilities();
  const { isPersonalMode } = useSystemConfig();
  const { subscribe } = useWebSocket();
  const { isOpen, dialogProps, confirm } = useConfirm();
  const [isUserMenuOpen, setIsUserMenuOpen] = useState(false);
  const [isFeedbackOpen, setIsFeedbackOpen] = useState(false);
  const [sessions, setSessions] = useState([]);
  const [isMobile, setIsMobile] = useState(false);
  const [showPinnedOnly, setShowPinnedOnly] = useState(false);
  const [workspaces, setWorkspaces] = useState([]);
  const [optimisticActiveSession, setOptimisticActiveSession] = useState(null);

  // Determine active page based on current route
  const isChatsActive = location.pathname === '/chats' || location.pathname === '/chat' || location.pathname.startsWith('/chat/');
  const isDashboardsActive = location.pathname.startsWith('/dashboard');
  const isKnowledgeActive = location.pathname.startsWith('/knowledge');
  const isWatchesActive = location.pathname.startsWith('/watches');
  const isSQLEditorActive = location.pathname === '/sql-editor';

  // Extract current session ID from URL (e.g., /chat/session-id-123 -> session-id-123)
  const activeSessionId = location.pathname.startsWith('/chat/')
    ? location.pathname.split('/chat/')[1]
    : null;

  // Fetch unread alerts count for Watches badge
  const { data: unreadAlertsCount = 0 } = useQuery({
    queryKey: ['unread-alerts-count'],
    queryFn: async () => {
      try {
        const response = await apiClient.get('/api/v1/watches/alerts/count');
        return response.data?.count || 0;
      } catch (error) {
        // Users without kyomi_watch capability will get 403 - show 0
        if (error.response?.status === 403) {
          return 0;
        }
        throw error;
      }
    },
    refetchInterval: 60000, // Refresh every minute
    staleTime: 30000,
  });

  // Fetch default dashboard for navigation
  const { data: defaultDashboardData } = useQuery({
    queryKey: ['default-dashboard'],
    queryFn: async () => {
      const response = await apiClient.get('/api/v1/workspaces/default-dashboard');
      return response.data;
    },
    staleTime: 60000, // Cache for 1 minute
  });

  // Detect mobile screen size and initialize
  useEffect(() => {
    const checkMobile = () => {
      const mobile = window.innerWidth < 768;
      setIsMobile(mobile);
      // Always collapse on mobile
      if (mobile) {
        setIsSidebarCollapsed(true);
      }
    };

    checkMobile();
    window.addEventListener('resize', checkMobile);
    return () => window.removeEventListener('resize', checkMobile);
  }, [setIsSidebarCollapsed]);

  // Auto-collapse sidebar on mobile after navigation
  useEffect(() => {
    if (isMobile && !isSidebarCollapsed) {
      setIsSidebarCollapsed(true);
    }
  }, [location.pathname, isMobile]);

  // Load sessions for recent chats
  useEffect(() => {
    loadSessions();
  }, []);

  // Subscribe to WebSocket events for real-time session updates
  useEffect(() => {

    // Subscribe to session_created events
    const unsubscribeSessionCreated = subscribe('session_created', (message) => {
      if (message.session_id && message.data) {
        setSessions(prevSessions => {
          const exists = prevSessions.some(s => s.session_id === message.session_id);
          if (exists) {
            return prevSessions;
          }
          return [message.data, ...prevSessions];
        });
      }
    });

    // Subscribe to title_update events
    const unsubscribeTitleUpdate = subscribe('title_update', (message) => {
      if (message.session_id && message.data?.title) {
        setSessions(prevSessions =>
          prevSessions.map(session =>
            session.session_id === message.session_id
              ? { ...session, title: message.data.title }
              : session
          )
        );
      }
    });

    // Cleanup - unsubscribe from all events on unmount
    return () => {
      unsubscribeSessionCreated();
      unsubscribeTitleUpdate();
    };
  }, [subscribe]);

  // Listen for session deletions from other components (e.g., ChatsList bulk delete)
  useEffect(() => {
    const handleSessionsDeleted = (event) => {
      const { sessionIds, source } = event.detail;
      if (source === 'sidebar') return; // Ignore our own events
      if (sessionIds?.length) {
        const deletedSet = new Set(sessionIds);
        setSessions(prev => prev.filter(s => !deletedSet.has(s.session_id)));
      }
    };

    window.addEventListener('sessions-deleted', handleSessionsDeleted);
    return () => window.removeEventListener('sessions-deleted', handleSessionsDeleted);
  }, []);

  // Fetch user's workspaces
  useEffect(() => {
    const fetchWorkspaces = async () => {
      if (!apiClient) return;

      try {
        const response = await apiClient.get('/api/v1/workspaces/my-workspaces');
        setWorkspaces(response.data || []);
      } catch (error) {
        setWorkspaces([]);
      }
    };

    fetchWorkspaces();
  }, [apiClient, user]);

  const loadSessions = async (pinnedOnly = showPinnedOnly) => {
    try {
      const sessions = await apiClient.getChatSessions(pinnedOnly);
      setSessions(sessions || []);
    } catch (error) {
      setSessions([]);
    }
  };

  const startNewChat = () => {
    navigate('/chat');
  };

  const switchToSession = (sessionId) => {
    // Optimistically update highlight for instant feedback
    setOptimisticActiveSession(sessionId);
    navigate(`/chat/${sessionId}`);
  };

  // Reset optimistic state when actual navigation completes
  useEffect(() => {
    if (activeSessionId) {
      setOptimisticActiveSession(null);
    }
  }, [activeSessionId]);

  const handleDeleteClick = async (sessionId, e) => {
    e.stopPropagation();

    const confirmed = await confirm({
      title: 'Delete Chat?',
      message: 'Are you sure you want to delete this chat? This action cannot be undone.',
      confirmText: 'Delete',
      variant: 'destructive'
    });

    if (confirmed) {
      try {
        await apiClient.deleteSession(sessionId);
        setSessions(sessions.filter(s => s.session_id !== sessionId));
        window.dispatchEvent(new CustomEvent('sessions-deleted', {
          detail: { sessionIds: [sessionId], source: 'sidebar' }
        }));
        toast.success('Chat deleted successfully');
      } catch (error) {
        if (error.response?.status === 404) {
          toast.error('Cannot delete this chat. Only the creator can delete shared chats.');
        } else {
          toast.error('Failed to delete chat. Please try again.');
        }
      }
    }
  };

  const handleLogout = async () => {
    try {
      await logout();
      navigate('/login');
    } catch (error) {
    }
  };

  const handleWorkspaceSwitch = async (workspaceId) => {
    try {
      await apiClient.post(`/api/v1/auth/switch-workspace/${workspaceId}`);
      // Reload the page to refresh with new workspace context
      window.location.reload();
    } catch (error) {
      toast.error(`Failed to switch workspace: ${error.response?.data?.detail || error.message}`);
    }
  };

  return (
    <>
      {/* Mobile Header Bar - only visible on mobile */}
      <div className="md:hidden fixed top-0 left-0 right-0 h-16 bg-background border-b border-border z-40 flex items-center px-4">
        <button
          onClick={() => setIsSidebarCollapsed(!isSidebarCollapsed)}
          className="p-2 hover:bg-accent rounded-lg transition-colors relative z-10"
          aria-label="Toggle menu"
        >
          <svg className="w-5 h-5 text-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 6h16M4 12h16M4 18h16" />
          </svg>
        </button>
        <div className="absolute left-1/2 -translate-x-1/2">
          <img src="/kyomi_full_logo.svg" alt="Kyomi" className="h-10 dark:hidden" />
          <img src="/kyomi_full_logo_white.svg" alt="Kyomi" className="h-10 hidden dark:block" />
        </div>
      </div>

      {/* Mobile Overlay - shown when sidebar is open on mobile */}
      {isMobile && !isSidebarCollapsed && (
        <div
          className="modal-overlay z-20 md:hidden"
          onClick={() => setIsSidebarCollapsed(true)}
        />
      )}

      <div
        className={`${
          isSidebarCollapsed ? 'w-16' : 'w-80'
        } bg-background border-r border-border text-foreground flex-col z-30 absolute left-0 transition-all duration-300 ease-in-out ${isMobile && isSidebarCollapsed ? 'hidden' : 'flex'} ${isMobile ? 'top-16 bottom-0' : 'inset-y-0'}`}
        style={{
          width: isSidebarCollapsed ? '4rem' : '20rem',
          flexDirection: 'column',
          position: 'absolute',
          left: 0,
          zIndex: 30,
          transition: 'all 300ms cubic-bezier(0.4, 0, 0.2, 1)'
        }}
      >
      {/* Header - hidden on mobile */}
      <div className="hidden md:flex px-3 h-16 border-b border-border items-center justify-between">
          <div className="flex items-center">
            <button
              onClick={() => setIsSidebarCollapsed(!isSidebarCollapsed)}
              className="p-2.5 hover:bg-accent rounded-md transition-colors flex-shrink-0"
            >
              <svg className="w-5 h-5 text-muted-foreground" viewBox="0 0 256 256" fill="currentColor">
                <path d="M216,40H40A16,16,0,0,0,24,56V200a16,16,0,0,0,16,16H216a16,16,0,0,0,16-16V56A16,16,0,0,0,216,40ZM40,152H56a8,8,0,0,0,0-16H40V120H56a8,8,0,0,0,0-16H40V88H56a8,8,0,0,0,0-16H40V56H80V200H40Zm176,48H96V56H216V200Z"/>
              </svg>
            </button>
            <div className={`flex items-center overflow-hidden transition-all duration-300 ${isSidebarCollapsed ? 'opacity-0 w-0 ml-0' : 'opacity-100 ml-2'}`}>
              <img src="/kyomi_full_logo.svg" alt="Kyomi" className="h-12 dark:hidden" />
              <img src="/kyomi_full_logo_white.svg" alt="Kyomi" className="h-12 hidden dark:block" />
            </div>
          </div>
      </div>


      {/* Navigation */}
      <div className={`flex-1 flex flex-col px-3 py-4 min-h-0 ${isSidebarCollapsed ? 'overflow-hidden' : 'overflow-x-hidden'}`}>
        <div className="space-y-1 mb-4">
          {/* New Chat Button */}
          <button
            onClick={startNewChat}
            className={`w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors ${
              isSidebarCollapsed ? 'gap-3 px-2.5' : 'gap-3 pl-2.5 pr-3 py-2.5'
            }`}
          >
            <div className="w-5 h-5 rounded-full flex items-center justify-center bg-primary flex-shrink-0">
              <svg className="w-3 h-3 text-primary-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="3">
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
              </svg>
            </div>
            <span className={`text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300 ${isSidebarCollapsed ? 'opacity-0' : 'opacity-100'}`}>
              New chat
            </span>
          </button>

          {/* Chats */}
          <button
            onClick={() => navigate('/chats')}
            className={`w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors ${
              isSidebarCollapsed ? 'gap-3 px-2.5' : 'gap-3 pl-2.5 pr-3 py-2.5'
            } ${isChatsActive ? 'bg-accent' : ''}`}
          >
            <svg className="w-5 h-5 text-muted-foreground flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
            </svg>
            <span className={`text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300 ${isSidebarCollapsed ? 'opacity-0' : 'opacity-100'}`}>
              Chats
            </span>
          </button>

          {/* Dashboards */}
          <button
            onClick={() => {
              // User preference > workspace default > dashboards list
              const userDefault = user?.extra_metadata?.default_dashboard_id;
              const workspaceDefault = defaultDashboardData?.default_dashboard_id;
              const effectiveId = userDefault || workspaceDefault;
              if (effectiveId) {
                navigate(`/dashboard/${effectiveId}`);
              } else {
                navigate('/dashboards');
              }
            }}
            className={`w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors ${
              isSidebarCollapsed ? 'gap-3 px-2.5' : 'gap-3 pl-2.5 pr-3 py-2.5'
            } ${isDashboardsActive ? 'bg-accent' : ''}`}
          >
            <svg className="w-5 h-5 text-muted-foreground flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
            </svg>
            <span className={`text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300 ${isSidebarCollapsed ? 'opacity-0' : 'opacity-100'}`}>
              Dashboards
            </span>
          </button>

          {/* Watches (Kyomi Watch) */}
          <button
            onClick={() => navigate('/watches')}
            className={`w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors ${
              isSidebarCollapsed ? 'gap-3 px-2.5' : 'gap-3 pl-2.5 pr-3 py-2.5'
            } ${isWatchesActive ? 'bg-accent' : ''}`}
          >
            <div className="relative flex-shrink-0">
              <svg className="w-5 h-5 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
              </svg>
              {unreadAlertsCount > 0 && isSidebarCollapsed && (
                <span className="absolute -top-1 -right-1 h-2 w-2 rounded-full bg-primary" />
              )}
            </div>
            <span className={`text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300 ${isSidebarCollapsed ? 'opacity-0' : 'opacity-100'}`}>
              Watches
            </span>
            {unreadAlertsCount > 0 && !isSidebarCollapsed && (
              <span className="ml-auto px-1.5 py-0.5 text-xs font-medium rounded-full bg-primary text-primary-foreground">
                {unreadAlertsCount > 99 ? '99+' : unreadAlertsCount}
              </span>
            )}
          </button>

          {/* Knowledge */}
          <button
            onClick={() => navigate('/knowledge')}
            className={`w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors ${
              isSidebarCollapsed ? 'gap-3 px-2.5' : 'gap-3 pl-2.5 pr-3 py-2.5'
            } ${isKnowledgeActive ? 'bg-accent' : ''}`}
          >
            <svg className="w-5 h-5 text-muted-foreground flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 6.253v13m0-13C10.832 5.477 9.246 5 7.5 5S4.168 5.477 3 6.253v13C4.168 18.477 5.754 18 7.5 18s3.332.477 4.5 1.253m0-13C13.168 5.477 14.754 5 16.5 5c1.747 0 3.332.477 4.5 1.253v13C19.832 18.477 18.247 18 16.5 18c-1.746 0-3.332.477-4.5 1.253" />
            </svg>
            <span className={`text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300 ${isSidebarCollapsed ? 'opacity-0' : 'opacity-100'}`}>
              Knowledge
            </span>
          </button>

          {/* SQL Editor */}
          <button
            onClick={() => navigate('/sql-editor')}
            className={`w-full h-10 flex items-center rounded-lg hover:bg-accent transition-colors ${
              isSidebarCollapsed ? 'gap-3 px-2.5' : 'gap-3 pl-2.5 pr-3 py-2.5'
            } ${isSQLEditorActive ? 'bg-accent' : ''}`}
          >
            <svg className="w-5 h-5 text-muted-foreground flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4m0 5c0 2.21-3.582 4-8 4s-8-1.79-8-4" />
            </svg>
            <span className={`text-sm font-medium text-foreground whitespace-nowrap overflow-hidden transition-opacity duration-300 ${isSidebarCollapsed ? 'opacity-0' : 'opacity-100'}`}>
              SQL Editor
            </span>
          </button>
        </div>

        {/* Recent Chats Section */}
        <div className={`border-t border-border pt-4 flex-1 flex flex-col min-h-0 transition-opacity duration-300 ${isSidebarCollapsed ? 'opacity-0 pointer-events-none' : 'opacity-100'}`}>
              <div className="flex items-center justify-between px-3 mb-2">
                <div className="text-xs text-muted-foreground font-medium opacity-100 transition-opacity duration-300">Recent Chats</div>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      onClick={() => {
                        const newFilter = !showPinnedOnly;
                        setShowPinnedOnly(newFilter);
                        loadSessions(newFilter);
                      }}
                      className={`p-1 rounded transition-colors ${
                        showPinnedOnly ? 'text-foreground' : 'text-muted-foreground hover:text-foreground'
                      }`}
                      aria-label={showPinnedOnly ? 'Show all chats' : 'Show only chats with pinned messages'}
                    >
                      <svg className="w-4 h-4" fill={showPinnedOnly ? 'currentColor' : 'none'} stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                      </svg>
                    </button>
                  </TooltipTrigger>
                  <TooltipContent>{showPinnedOnly ? 'Show all chats' : 'Show only chats with pinned messages'}</TooltipContent>
                </Tooltip>
              </div>
              <div className="space-y-1 overflow-y-auto">
                {sessions.length > 0 ? (
                  sessions.map((session) => {
                    // Use optimistic state for instant highlight, fallback to actual URL state
                    const isActive = (optimisticActiveSession === session.session_id) ||
                                    (optimisticActiveSession === null && activeSessionId === session.session_id);
                    return (
                      <div
                        key={session.session_id}
                        onClick={() => switchToSession(session.session_id)}
                        className={`group relative px-3 py-2 rounded-lg cursor-pointer transition-all duration-300 text-sm opacity-100 hover:bg-accent text-foreground ${
                          isActive ? 'bg-accent' : ''
                        }`}
                        onMouseEnter={(e) => {
                          // Show delete button on hover
                          const btn = e.currentTarget.querySelector('[data-delete-button]');
                          if (btn) btn.style.opacity = '1';
                        }}
                        onMouseLeave={(e) => {
                          // Hide delete button when not hovering
                          const btn = e.currentTarget.querySelector('[data-delete-button]');
                          if (btn) btn.style.opacity = '0';
                        }}
                      >
                        <div className="flex items-center gap-2 pr-8">
                          <div className="font-normal truncate flex-1">
                            {session.title || 'New Chat'}
                          </div>
                          {/* Show Slack badge for shared Slack threads (channel mentions), Shared badge for shared chats (team plan only), nothing for private/DMs */}
                          {session.slack_channel_id && session.shared ? (
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <div className="flex-shrink-0">
                                  <Badge variant="outline" className="flex items-center gap-1 px-1.5 py-0.5 text-xs">
                                    <svg className="w-3 h-3" viewBox="0 0 24 24" fill="currentColor">
                                      <path d="M6 15a2 2 0 0 1-2 2a2 2 0 0 1-2-2a2 2 0 0 1 2-2h2v2zm1 0a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-5zm2-8a2 2 0 0 1-2-2a2 2 0 0 1 2-2a2 2 0 0 1 2 2v2H9zm0 1a2 2 0 0 1 2 2a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5zm8 2a2 2 0 0 1 2-2a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-2v-2zm-1 0a2 2 0 0 1-2 2a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5zm-2 8a2 2 0 0 1 2 2a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-2h2zm0-1a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-5z"/>
                                    </svg>
                                  </Badge>
                                </div>
                              </TooltipTrigger>
                              <TooltipContent>Synced with Slack</TooltipContent>
                            </Tooltip>
                          ) : capabilities.multiUserEnabled && session.shared ? (
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <div className="flex-shrink-0">
                                  <Badge variant="default" className="flex items-center gap-1 px-1.5 py-0.5 text-xs">
                                    <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8.684 13.342C8.886 12.938 9 12.482 9 12c0-.482-.114-.938-.316-1.342m0 2.684a3 3 0 110-2.684m0 2.684l6.632 3.316m-6.632-6l6.632-3.316m0 0a3 3 0 105.367-2.684 3 3 0 00-5.367 2.684zm0 9.316a3 3 0 105.368 2.684 3 3 0 00-5.368-2.684z" />
                                    </svg>
                                  </Badge>
                                </div>
                              </TooltipTrigger>
                              <TooltipContent>Shared with workspace</TooltipContent>
                            </Tooltip>
                          ) : null}
                        </div>
                        {/* Only show delete button if user owns this session */}
                        {(() => {
                          const canDelete = session.created_by?.user_id === user?.user_id;
                          return canDelete;
                        })() && (
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <button
                                data-delete-button
                                onClick={(e) => handleDeleteClick(session.session_id, e)}
                                className="absolute right-2 top-1/2 transform -translate-y-1/2 p-1 opacity-0 group-hover:opacity-100 hover:bg-error/20 rounded transition-all"
                                style={{ opacity: 0, transition: 'opacity 150ms' }}
                                aria-label="Delete chat"
                              >
                                <svg className="w-4 h-4 text-error-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                                </svg>
                              </button>
                            </TooltipTrigger>
                            <TooltipContent>Delete chat</TooltipContent>
                          </Tooltip>
                        )}
                      </div>
                    );
                  })
                ) : (
                  <div className="text-xs text-muted-foreground px-3 py-2 italic opacity-100 transition-opacity duration-300">No chats yet</div>
                )}
              </div>
        </div>
      </div>

      {/* User Account Section */}
      <div className="border-t border-border px-3 py-4 relative">
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              onClick={() => setIsUserMenuOpen(!isUserMenuOpen)}
              className={`flex items-center w-full h-10 hover:bg-accent rounded-lg transition-colors ${
                isSidebarCollapsed ? 'gap-3 px-2' : 'gap-3 pl-2 pr-3 py-2.5'
              }`}
              aria-label={isPersonalMode ? 'Settings' : (user?.name || user?.email || 'User')}
            >
              <div className="w-6 h-6 bg-primary rounded-full flex items-center justify-center flex-shrink-0">
                <span className="text-xs font-medium text-primary-foreground">
                  {isPersonalMode ? '⚙' : (user?.name?.charAt(0) || user?.email?.charAt(0) || 'U')}
                </span>
              </div>
              <div className={`flex-1 min-w-0 text-left overflow-hidden transition-all duration-300 ${isSidebarCollapsed ? 'opacity-0 w-0' : 'opacity-100'}`}>
                <div className="text-sm font-medium text-foreground truncate">{isPersonalMode ? 'Settings' : (user?.name || user?.email || 'User')}</div>
                {!isPersonalMode && (
                <div className="text-xs text-muted-foreground truncate">
                  {user?.workspace_name || 'My Workspace'}
                </div>
                )}
              </div>
              <svg className={`w-4 h-4 text-muted-foreground flex-shrink-0 transition-all duration-300 ${isUserMenuOpen ? 'rotate-180' : ''} ${isSidebarCollapsed ? 'opacity-0 w-0' : 'opacity-100'}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
              </svg>
            </button>
          </TooltipTrigger>
          <TooltipContent>{user?.name || user?.email || 'User'}</TooltipContent>
        </Tooltip>

        {isUserMenuOpen && (
          <div className="absolute bottom-full left-0 mb-2 bg-popover border border-border rounded-lg shadow-lg py-1 z-50 min-w-48">
            {/* User Info Header — hide email in personal mode */}
            {!isPersonalMode && (
            <div className="px-4 py-3 border-b border-border">
              <div className="text-sm font-medium text-popover-foreground">{user?.name || 'User'}</div>
              <div className="text-xs text-muted-foreground truncate">{user?.email}</div>
            </div>
            )}
            {/* Workspace Switcher - only show if user belongs to 2+ workspaces, hidden in personal mode */}
            {!isPersonalMode && workspaces.length > 1 && (
              <WorkspaceSwitcher
                workspaces={workspaces}
                currentWorkspaceId={user?.workspace_id}
                onSwitch={handleWorkspaceSwitch}
                onClose={() => setIsUserMenuOpen(false)}
              />
            )}
            <button
              onClick={() => {
                navigate('/settings');
                setIsUserMenuOpen(false);
              }}
              className="w-full text-left px-4 py-2 text-sm text-popover-foreground hover:bg-accent flex items-center space-x-3"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
              </svg>
              <span>Settings</span>
            </button>
            <a
              href="https://kyomi.ai/docs"
              target="_blank"
              rel="noopener noreferrer"
              onClick={() => setIsUserMenuOpen(false)}
              className="w-full text-left px-4 py-2 text-sm text-popover-foreground hover:bg-accent flex items-center space-x-3"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 6.253v13m0-13C10.832 5.477 9.246 5 7.5 5S4.168 5.477 3 6.253v13C4.168 18.477 5.754 18 7.5 18s3.332.477 4.5 1.253m0-13C13.168 5.477 14.754 5 16.5 5c1.747 0 3.332.477 4.5 1.253v13C19.832 18.477 18.247 18 16.5 18c-1.746 0-3.332.477-4.5 1.253" />
              </svg>
              <span>Help & Docs</span>
            </a>
            <button
              onClick={() => {
                setIsFeedbackOpen(true);
                setIsUserMenuOpen(false);
              }}
              className="w-full text-left px-4 py-2 text-sm text-popover-foreground hover:bg-accent flex items-center space-x-3"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
              </svg>
              <span>Send Feedback</span>
            </button>
            {!isPersonalMode && (
              <button
                onClick={() => {
                  handleLogout();
                  setIsUserMenuOpen(false);
                }}
                className="w-full text-left px-4 py-2 text-sm text-error-foreground hover:bg-error/10 flex items-center space-x-3"
              >
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M17 16l4-4m0 0l-4-4m4 4H7m6 4v1a3 3 0 01-3 3H6a3 3 0 01-3-3V7a3 3 0 013-3h4a3 3 0 013 3v1" />
                </svg>
                <span>Logout</span>
              </button>
            )}
          </div>
        )}
      </div>
    </div>

    {/* Confirm Dialog */}
    <ConfirmDialog isOpen={isOpen} {...dialogProps} />

    {/* Feedback Modal */}
    <FeedbackModal open={isFeedbackOpen} onOpenChange={setIsFeedbackOpen} />
    </>
  );
};

export default Sidebar;
