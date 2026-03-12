// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { useAuth } from '../context/AuthContext';
import { useCapabilities } from '../context/CapabilitiesContext';
import { useWebSocket } from '../context/WebSocketContext';
import { Tooltip, TooltipTrigger, TooltipContent } from '../components/ui/tooltip';
import { Spinner } from '../components/ui/spinner';
import { Badge } from '../components/ui/badge';
import { Checkbox } from '../components/ui/checkbox';
import ConfirmDialog from '../components/ConfirmDialog';
import useConfirm from '../hooks/useConfirm';

const ChatsList = () => {
  const { isOpen, dialogProps, confirm } = useConfirm();
  const navigate = useNavigate();
  const { apiClient, user } = useAuth();
  const capabilities = useCapabilities();
  const { subscribe } = useWebSocket();
  const [sessions, setSessions] = useState([]);
  const [searchQuery, setSearchQuery] = useState('');
  const [isLoading, setIsLoading] = useState(true);
  const [isSearching, setIsSearching] = useState(false);
  const [showPinnedOnly, setShowPinnedOnly] = useState(false);
  const [chatFilter, setChatFilter] = useState('all'); // 'all', 'mine', 'shared_with_me', 'slack'
  const [selectedChats, setSelectedChats] = useState(new Set());
  const [isBulkDeleting, setIsBulkDeleting] = useState(false);

  // Load all chat sessions on mount
  useEffect(() => {
    loadSessions();
  }, []);

  // Subscribe to WebSocket updates for shared conversation activity
  useEffect(() => {
    const unsubscribe = subscribe('shared_conversation_activity', (message) => {

      // Update session with new activity and re-sort to move it to top
      setSessions(prevSessions => {
        const updatedSessions = prevSessions.map(session => {
          if (session.session_id === message.session_id) {
            return {
              ...session,
              updated_at: message.timestamp, // Use the timestamp from the message
              unread_count: (session.unread_count || 0) + 1, // Increment unread count
              last_activity_at: message.timestamp
            };
          }
          return session;
        });

        // Re-sort sessions by updated_at so the active conversation moves to top
        return updatedSessions.sort((a, b) =>
          new Date(b.updated_at) - new Date(a.updated_at)
        );
      });
    });

    return () => unsubscribe();
  }, [subscribe]);

  // Debounced search effect
  useEffect(() => {
    // If search query is empty, load all sessions
    if (!searchQuery.trim()) {
      setIsSearching(false);
      loadSessions();
      return;
    }

    // Debounce the search API call
    const timeoutId = setTimeout(() => {
      // Show searching indicator only when we're about to search
      setIsSearching(true);
      performSearch(searchQuery);
    }, 300); // 300ms debounce

    return () => clearTimeout(timeoutId);
  }, [searchQuery]);

  // Reload sessions when pinned filter changes
  useEffect(() => {
    if (!searchQuery.trim()) {
      loadSessions();
    } else {
      performSearch(searchQuery);
    }
  }, [showPinnedOnly]);

  // Clear selection when filter or search changes
  useEffect(() => {
    setSelectedChats(new Set());
  }, [chatFilter, searchQuery, showPinnedOnly]);

  // Listen for session deletions from other components (e.g., Sidebar)
  useEffect(() => {
    const handleSessionsDeleted = (event) => {
      const { sessionIds, source } = event.detail;
      if (source === 'chatsList') return; // Ignore our own events
      if (sessionIds?.length) {
        const deletedSet = new Set(sessionIds);
        setSessions(prev => prev.filter(s => !deletedSet.has(s.session_id)));
        setSelectedChats(prev => {
          const updated = new Set(prev);
          sessionIds.forEach(id => updated.delete(id));
          return updated.size !== prev.size ? updated : prev;
        });
      }
    };

    window.addEventListener('sessions-deleted', handleSessionsDeleted);
    return () => window.removeEventListener('sessions-deleted', handleSessionsDeleted);
  }, []);

  const loadSessions = useCallback(async (pinnedOnly = showPinnedOnly) => {
    try {
      // Only show full loading state on initial load
      if (sessions.length === 0) {
        setIsLoading(true);
      }
      const data = await apiClient.getChatSessions(pinnedOnly);
      setSessions(data || []);
    } catch (error) {
      setSessions([]);
    } finally {
      setIsLoading(false);
    }
  }, [showPinnedOnly, sessions.length]); // eslint-disable-line react-hooks/exhaustive-deps

  const performSearch = useCallback(async (query) => {
    try {
      // Don't set isLoading - use isSearching instead to avoid flash
      const data = await apiClient.searchChatMessages(query);
      let results = data.sessions || [];

      // Apply pinned filter to search results if enabled
      if (showPinnedOnly) {
        results = results.filter(session => session.pinned_count > 0);
      }

      setSessions(results);
    } catch (error) {
      setSessions([]);
    } finally {
      setIsSearching(false);
    }
  }, [showPinnedOnly]); // eslint-disable-line react-hooks/exhaustive-deps

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
          detail: { sessionIds: [sessionId], source: 'chatsList' }
        }));
      } catch (error) {
      }
    }
  };

  const formatDate = (dateString) => {
    const date = new Date(dateString);
    const now = new Date();
    const diffInMs = now - date;
    const diffInMins = Math.floor(diffInMs / (1000 * 60));
    const diffInHours = Math.floor(diffInMins / 60);
    const diffInDays = Math.floor(diffInHours / 24);

    if (diffInMins < 1) return 'Just now';
    if (diffInMins < 60) return `${diffInMins}m ago`;
    if (diffInHours < 24) return `${diffInHours}h ago`;
    if (diffInDays < 7) return `${diffInDays}d ago`;

    return date.toLocaleDateString('en-US', {
      month: 'short',
      day: 'numeric',
      year: date.getFullYear() !== now.getFullYear() ? 'numeric' : undefined
    });
  };

  // Get status of a chat session for the current user
  const getChatStatus = (session) => {
    const sessionIsOwned = session.user_id === user?.user_id ||
                    session.created_by?.user_id === user?.user_id ||
                    (!session.created_by && !session.shared); // Legacy sessions

    if (sessionIsOwned && session.shared) {
      return 'shared_by_me'; // I created it and shared it
    } else if (sessionIsOwned && !session.shared) {
      return 'private'; // I created it, not shared
    } else {
      return 'shared_with_me'; // Someone else created it and shared with me
    }
  };

  // Filter and sort sessions based on current filter
  const getFilteredSessions = () => {
    let filtered = sessions;

    // Apply chat filter
    if (chatFilter === 'mine') {
      filtered = sessions.filter(session => {
        const status = getChatStatus(session);
        return status === 'private' || status === 'shared_by_me';
      });
    } else if (chatFilter === 'shared_with_me') {
      filtered = sessions.filter(session => getChatStatus(session) === 'shared_with_me');
    } else if (chatFilter === 'slack') {
      // Only show Slack conversations that are shared (channel mentions)
      // DMs have slack_channel_id but shared=false, so they appear in "Private" instead
      filtered = sessions.filter(session => session.slack_channel_id && session.shared);
    }
    // 'all' shows everything, no filtering needed

    // Sort by last activity (most recent first)
    return filtered.sort((a, b) => {
      const dateA = new Date(a.last_activity_at || a.created_at);
      const dateB = new Date(b.last_activity_at || b.created_at);
      return dateB - dateA;
    });
  };

  // Bulk selection helpers
  const isOwned = (session) => {
    return session.created_by?.user_id === user?.user_id ||
      session.user_id === user?.user_id ||
      (!session.created_by && !session.shared);
  };

  const filteredSessions = getFilteredSessions();
  const selectableSessions = filteredSessions.filter(s => isOwned(s));
  const hasSelection = selectedChats.size > 0;
  const isAllSelected = selectableSessions.length > 0 && selectedChats.size === selectableSessions.length;
  const isIndeterminate = selectedChats.size > 0 && selectedChats.size < selectableSessions.length;

  const toggleSelectAll = () => {
    if (isAllSelected) {
      setSelectedChats(new Set());
    } else {
      setSelectedChats(new Set(selectableSessions.map(s => s.session_id)));
    }
  };

  const toggleChatSelection = (sessionId) => {
    const newSelected = new Set(selectedChats);
    if (newSelected.has(sessionId)) {
      newSelected.delete(sessionId);
    } else {
      newSelected.add(sessionId);
    }
    setSelectedChats(newSelected);
  };

  const handleBulkDelete = async () => {
    if (selectedChats.size === 0) return;

    const confirmed = await confirm({
      title: `Delete ${selectedChats.size} chat${selectedChats.size !== 1 ? 's' : ''}?`,
      message: 'Are you sure you want to delete these chats? This action cannot be undone.',
      confirmText: 'Delete',
      variant: 'destructive'
    });

    if (confirmed) {
      setIsBulkDeleting(true);
      try {
        const deletedIds = [...selectedChats];
        await apiClient.bulkDeleteSessions(deletedIds);
        setSessions(prev => prev.filter(s => !selectedChats.has(s.session_id)));
        setSelectedChats(new Set());
        window.dispatchEvent(new CustomEvent('sessions-deleted', {
          detail: { sessionIds: deletedIds, source: 'chatsList' }
        }));
      } catch (error) {
        // Error handled by apiClient interceptors
      } finally {
        setIsBulkDeleting(false);
      }
    }
  };

  // Render a single session item
  const renderSessionItem = (session) => {
    const status = getChatStatus(session);
    const owned = isOwned(session);
    const isSelected = selectedChats.has(session.session_id);

    return (
      <div
        key={session.session_id}
        className={`group flex items-center gap-3 bg-card border rounded-lg hover:border-border hover:shadow-sm transition-all ${
          isSelected ? 'border-primary/50 ring-2 ring-primary/20' : 'border-border'
        }`}
      >
        {/* Checkbox for owned sessions */}
        {owned && (
          <div className="pl-4 flex items-center shrink-0">
            <Checkbox
              checked={isSelected}
              onCheckedChange={() => toggleChatSelection(session.session_id)}
            />
          </div>
        )}

        <div
          onClick={() => navigate(`/chat/${session.session_id}`)}
          className={`flex-1 min-w-0 p-4 cursor-pointer ${owned ? 'pl-0' : ''}`}
          onMouseEnter={(e) => {
            const buttons = e.currentTarget.querySelectorAll('[data-hover-button]');
            buttons.forEach(btn => btn.style.opacity = '1');
          }}
          onMouseLeave={(e) => {
            const buttons = e.currentTarget.querySelectorAll('[data-hover-button]');
            buttons.forEach(btn => btn.style.opacity = '0');
          }}
        >
          <div className="flex items-start justify-between gap-4">
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2 mb-1">
                <h3 className="text-base font-medium text-foreground truncate">
                  {session.title || 'Untitled Chat'}
                </h3>

                {/* Status badge - Slack badge for shared channel mentions, Shared/Private for DMs and regular chats */}
                {session.slack_channel_id && session.shared ? (
                  <Badge variant="outline" className="flex items-center gap-1">
                    <svg className="w-3 h-3" viewBox="0 0 24 24" fill="currentColor">
                      <path d="M6 15a2 2 0 0 1-2 2a2 2 0 0 1-2-2a2 2 0 0 1 2-2h2v2zm1 0a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-5zm2-8a2 2 0 0 1-2-2a2 2 0 0 1 2-2a2 2 0 0 1 2 2v2H9zm0 1a2 2 0 0 1 2 2a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5zm8 2a2 2 0 0 1 2-2a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-2v-2zm-1 0a2 2 0 0 1-2 2a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5zm-2 8a2 2 0 0 1 2 2a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-2h2zm0-1a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-5z"/>
                    </svg>
                    Slack
                  </Badge>
                ) : capabilities.multiUserEnabled ? (
                  <>
                    {status === 'private' && (
                      <Badge variant="secondary">Private</Badge>
                    )}
                    {status === 'shared_by_me' && (
                      <Badge variant="default">Shared</Badge>
                    )}
                    {status === 'shared_with_me' && (
                      <Badge variant="default">
                        {session.created_by?.display_name || 'Shared'}
                      </Badge>
                    )}
                  </>
                ) : null}

                {/* Unread count badge */}
                {session.unread_count > 0 && (
                  <Badge variant="warning">{session.unread_count} new</Badge>
                )}
              </div>
              <p className="text-sm text-muted-foreground">
                {formatDate(session.last_activity_at || session.created_at)}
              </p>
            </div>

            <div className="flex items-center gap-2">
              {/* Only show delete button if user owns this session */}
              {owned && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      data-hover-button
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDeleteClick(session.session_id, e);
                      }}
                      className="opacity-0 group-hover:opacity-100 p-2 text-muted-foreground hover:text-error-foreground hover:bg-error/10 rounded-lg transition-all"
                      style={{ opacity: 0, transition: 'opacity 150ms' }}
                      aria-label="Delete chat"
                    >
                      <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                      </svg>
                    </button>
                  </TooltipTrigger>
                  <TooltipContent>Delete chat</TooltipContent>
                </Tooltip>
              )}
            </div>
          </div>
        </div>
      </div>
    );
  };

  // Render unified session list
  const renderSessionsList = () => {
    if (filteredSessions.length === 0) {
      return (
        <div className="flex flex-col items-center justify-center py-12 text-muted-foreground">
          <svg className="w-16 h-16 mb-4 opacity-50" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
          </svg>
          <p className="text-lg font-medium">No conversations found</p>
          <p className="text-sm mt-1">Start a new chat to get started</p>
        </div>
      );
    }

    return (
      <div className="space-y-2">
        {filteredSessions.map(session => renderSessionItem(session))}
      </div>
    );
  };

  return (
    <div className="flex flex-col h-full bg-muted" style={{flexDirection: 'column'}}>
      {/* Header */}
      <div className="h-16 bg-card border-b border-border px-6 flex-shrink-0 flex items-center justify-between">
        <h1 className="text-2xl font-semibold text-foreground">Chats</h1>

        {/* New Chat Button */}
        <button
          onClick={() => navigate('/chat')}
          className="flex items-center gap-2 px-4 py-2 text-sm font-medium text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors"
        >
          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
          </svg>
          New Chat
        </button>
      </div>

      {/* Search and Filter Toolbar */}
      <div className="bg-card border-b border-border px-6 py-3 flex-shrink-0">
        {/* Search Bar and Pinned Filter */}
        <div className="flex items-center gap-3">
          {/* Select-all checkbox */}
          {selectableSessions.length > 0 && (
            <div className="flex items-center shrink-0">
              <Checkbox
                checked={isAllSelected}
                indeterminate={isIndeterminate}
                onCheckedChange={toggleSelectAll}
              />
            </div>
          )}

          <div className="relative flex-1">
            {/* Search icon or spinner */}
            {isSearching ? (
            <Spinner size="sm" className="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground" />
          ) : (
            <svg
              className="absolute left-3 top-1/2 transform -translate-y-1/2 w-4 h-4 text-muted-foreground"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
            </svg>
          )}
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Search chats..."
            className="w-full pl-9 pr-9 py-2 text-sm border border-border rounded-lg bg-card text-foreground focus:outline-none focus:ring-2 focus:ring-amber-500"
          />
          {searchQuery && !isSearching && (
            <button
              onClick={() => setSearchQuery('')}
              className="absolute right-3 top-1/2 transform -translate-y-1/2 text-muted-foreground hover:text-foreground"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          )}
          </div>

          {/* Pinned Filter Button - Simple star icon */}
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => setShowPinnedOnly(!showPinnedOnly)}
                className={`p-2 rounded-lg transition-colors ${
                  showPinnedOnly
                    ? 'bg-accent text-foreground'
                    : 'bg-card border border-border text-muted-foreground hover:text-foreground hover:bg-accent'
                }`}
                aria-label={showPinnedOnly ? 'Show all chats' : 'Show only pinned chats'}
              >
                <svg className="w-4 h-4" fill={showPinnedOnly ? 'currentColor' : 'none'} stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                </svg>
              </button>
            </TooltipTrigger>
            <TooltipContent>{showPinnedOnly ? 'Show all chats' : 'Show only pinned chats'}</TooltipContent>
          </Tooltip>
        </div>

        {/* Filter Buttons OR Bulk Action Bar */}
        {hasSelection ? (
          <div className="flex items-center gap-3 mt-3">
            <span className="text-sm font-medium text-foreground">
              {selectedChats.size} selected
            </span>
            <button
              onClick={handleBulkDelete}
              disabled={isBulkDeleting}
              className="flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium text-error-foreground bg-error/10 hover:bg-error/20 rounded-lg transition-colors disabled:opacity-50"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
              </svg>
              {isBulkDeleting ? 'Deleting...' : 'Delete'}
            </button>
            <button
              onClick={() => setSelectedChats(new Set())}
              className="ml-auto px-3 py-1.5 text-sm text-muted-foreground hover:text-foreground hover:bg-accent rounded-lg transition-colors"
            >
              Cancel
            </button>
          </div>
        ) : (
          <div className="flex items-center gap-2 mt-3">
            <button
              onClick={() => setChatFilter('all')}
              className={`px-3 py-1.5 text-sm rounded-lg transition-colors ${
                chatFilter === 'all'
                  ? 'bg-primary text-white'
                  : 'bg-accent text-foreground hover:bg-accent/80'
              }`}
            >
              All
            </button>
            {capabilities.multiUserEnabled && (
              <>
                <button
                  onClick={() => setChatFilter('mine')}
                  className={`px-3 py-1.5 text-sm rounded-lg transition-colors ${
                    chatFilter === 'mine'
                      ? 'bg-primary text-white'
                      : 'bg-accent text-foreground hover:bg-accent/80'
                  }`}
                >
                  My Conversations
                </button>
                <button
                  onClick={() => setChatFilter('shared_with_me')}
                  className={`px-3 py-1.5 text-sm rounded-lg transition-colors ${
                    chatFilter === 'shared_with_me'
                      ? 'bg-primary text-white'
                      : 'bg-accent text-foreground hover:bg-accent/80'
                  }`}
                >
                  Shared with Me
                </button>
              </>
            )}
            <button
              onClick={() => setChatFilter('slack')}
              className={`px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 ${
                chatFilter === 'slack'
                  ? 'bg-primary text-white'
                  : 'bg-accent text-foreground hover:bg-accent/80'
              }`}
            >
              <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="currentColor">
                <path d="M6 15a2 2 0 0 1-2 2a2 2 0 0 1-2-2a2 2 0 0 1 2-2h2v2zm1 0a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-5zm2-8a2 2 0 0 1-2-2a2 2 0 0 1 2-2a2 2 0 0 1 2 2v2H9zm0 1a2 2 0 0 1 2 2a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5zm8 2a2 2 0 0 1 2-2a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-2v-2zm-1 0a2 2 0 0 1-2 2a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5zm-2 8a2 2 0 0 1 2 2a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-2h2zm0-1a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-5z"/>
              </svg>
              Slack
            </button>
          </div>
        )}

        {/* Results Count */}
        {searchQuery && (
          <div className="text-sm text-muted-foreground mt-2">
            {sessions.length} {sessions.length === 1 ? 'chat' : 'chats'} matching "{searchQuery}"
          </div>
        )}
      </div>

      {/* Chats List */}
      <div className="flex-1 overflow-y-auto p-4 md:p-6">
        {isLoading ? (
          <div className="flex items-center justify-center py-12">
            <div className="text-muted-foreground">Loading chats...</div>
          </div>
        ) : sessions.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-12">
            <svg className="w-16 h-16 text-muted-foreground/50 mb-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
            </svg>
            <p className="text-muted-foreground text-lg mb-2">
              {searchQuery ? 'No chats found' : 'No chats yet'}
            </p>
            <p className="text-muted-foreground text-sm mb-4">
              {searchQuery ? 'Try a different search term' : 'Start a new conversation to get started'}
            </p>
            {!searchQuery && (
              <button
                onClick={() => navigate('/chat')}
                className="px-4 py-2 text-sm font-medium text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors"
              >
                Start New Chat
              </button>
            )}
          </div>
        ) : (
          <div className="max-w-4xl mx-auto" style={{display: 'block'}}>
            {renderSessionsList()}
          </div>
        )}
      </div>

      {/* Confirm Dialog */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
    </div>
  );
};

export default ChatsList;
