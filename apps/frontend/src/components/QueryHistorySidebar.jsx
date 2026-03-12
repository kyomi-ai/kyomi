// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useRef, useCallback } from 'react';
import apiClient from '../api/apiClient.js';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';
import { Spinner } from './ui/spinner';
import ConfirmDialog from './ConfirmDialog';
import useConfirm from '../hooks/useConfirm';
import { toast } from '../lib/toast';

/**
 * QueryHistorySidebar - Query history sidebar for SQL Editor
 *
 * Features:
 * - List query history with search
 * - Display query preview, execution metadata
 * - Save/star queries
 * - Click to load query into editor
 * - Delete queries
 * - Infinite scroll for large history lists
 */
const QueryHistorySidebar = ({
  onQuerySelect,
  searchQuery = '',
  onSearchChange,
  refreshTrigger = 0
}) => {
  const { isOpen, dialogProps, confirm } = useConfirm();
  const [queryHistory, setQueryHistory] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [showSavedOnly, setShowSavedOnly] = useState(false);

  // Infinite scroll state
  const offsetRef = useRef(0);
  const [hasMore, setHasMore] = useState(true);
  const [isLoadingMore, setIsLoadingMore] = useState(false);
  const observerTarget = useRef(null);
  const ITEMS_PER_PAGE = 50;

  const loadQueryHistory = useCallback(async (reset = false) => {
    try {
      // If resetting, show main loader. Otherwise show "loading more" state
      if (reset) {
        setLoading(true);
        offsetRef.current = 0;
        setHasMore(true);
      } else {
        setIsLoadingMore(true);
      }
      setError(null);

      const params = {
        limit: ITEMS_PER_PAGE,
        offset: reset ? 0 : offsetRef.current,
        saved_only: showSavedOnly
      };

      if (searchQuery) {
        params.search = searchQuery;
      }

      const response = await apiClient.get('/api/v1/sql/history', { params });
      const newData = response.data;

      // Update state
      if (reset) {
        setQueryHistory(newData);
        offsetRef.current = newData.length;
      } else {
        setQueryHistory(prev => [...prev, ...newData]);
        offsetRef.current += newData.length;
      }

      // Check if we have more data
      setHasMore(newData.length === ITEMS_PER_PAGE);

    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to load query history');
    } finally {
      setLoading(false);
      setIsLoadingMore(false);
    }
  }, [showSavedOnly, searchQuery, ITEMS_PER_PAGE]);

  // Load initial query history (reset list)
  useEffect(() => {
    loadQueryHistory(true);
  }, [showSavedOnly, searchQuery, refreshTrigger, loadQueryHistory]);

  // Infinite scroll observer
  useEffect(() => {
    const observer = new IntersectionObserver(
      (entries) => {
        // When sentinel comes into view and we have more data and not already loading
        if (entries[0].isIntersecting && hasMore && !isLoadingMore && !loading) {
          loadQueryHistory(false);
        }
      },
      { threshold: 0.1 } // Trigger when 10% of sentinel is visible
    );

    const currentTarget = observerTarget.current;
    if (currentTarget) {
      observer.observe(currentTarget);
    }

    return () => {
      if (currentTarget) {
        observer.unobserve(currentTarget);
      }
    };
  }, [hasMore, isLoadingMore, loading, loadQueryHistory]);

  // Toggle saved status
  const handleToggleSaved = async (queryId, currentlySaved, event) => {
    event.stopPropagation(); // Prevent query selection

    try {
      await apiClient.patch(`/api/v1/sql/history/${queryId}`, {
        is_saved: !currentlySaved
      });

      // Update local state
      setQueryHistory(prev =>
        prev.map(q =>
          q.query_id === queryId
            ? { ...q, is_saved: !currentlySaved }
            : q
        )
      );
    } catch (err) {
      toast.error('Failed to save query: ' + (err.response?.data?.detail || err.message));
    }
  };

  // Delete query
  const handleDeleteQuery = async (queryId, event) => {
    event.stopPropagation(); // Prevent query selection

    const confirmed = await confirm({
      title: 'Delete Query?',
      message: 'Delete this query from history?',
      confirmText: 'Delete',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }

    try {
      await apiClient.delete(`/api/v1/sql/history/${queryId}`);

      // Remove from local state
      setQueryHistory(prev => prev.filter(q => q.query_id !== queryId));
    } catch (err) {
      toast.error('Failed to delete query: ' + (err.response?.data?.detail || err.message));
    }
  };

  // Format timestamp
  const formatTimestamp = (isoString) => {
    const date = new Date(isoString);
    const now = new Date();
    const diffMs = now - date;
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMs / 3600000);
    const diffDays = Math.floor(diffMs / 86400000);

    if (diffMins < 1) return 'Just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays === 1) return 'Yesterday';
    if (diffDays < 7) return `${diffDays}d ago`;

    return date.toLocaleDateString();
  };

  // Format execution time
  const formatExecutionTime = (ms) => {
    if (!ms) return null;
    if (ms < 1000) return `${ms}ms`;
    return `${(ms / 1000).toFixed(1)}s`;
  };

  // Get query preview (first line, truncated)
  const getQueryPreview = (queryText) => {
    const firstLine = queryText.split('\n')[0].trim();
    if (firstLine.length > 60) {
      return firstLine.substring(0, 60) + '...';
    }
    return firstLine;
  };

  // Render content based on state
  const renderContent = () => {
    if (loading) {
      return (
        <div className="flex-1 flex items-center justify-center py-8">
          <Spinner size="md" className="text-primary" />
        </div>
      );
    }

    if (error) {
      return (
        <div className="flex-1 p-3">
          <Alert variant="error">
            <AlertDescription className="text-xs">{error}</AlertDescription>
          </Alert>
        </div>
      );
    }

    if (queryHistory.length === 0) {
      return (
        <div className="flex-1 flex items-center justify-center p-4">
          <div className="text-center">
            <div className="text-xs text-muted-foreground">
              {searchQuery
                ? 'No queries found'
                : showSavedOnly
                ? 'No saved queries yet'
                : 'No query history yet'}
            </div>
            {searchQuery && (
              <button
                onClick={() => onSearchChange('')}
                className="mt-2 text-xs text-primary hover:text-info-foreground"
              >
                Clear search
              </button>
            )}
            {showSavedOnly && !searchQuery && (
              <button
                onClick={() => setShowSavedOnly(false)}
                className="mt-2 text-xs text-primary hover:text-info-foreground"
              >
                Show all queries
              </button>
            )}
          </div>
        </div>
      );
    }

    return (
      <div className="flex-1 overflow-auto">
        {queryHistory.map((query) => (
        <Tooltip key={query.query_id}>
          <TooltipTrigger asChild>
            <div
              className="px-3 py-2 border-b border-border cursor-pointer transition-colors hover:bg-accent"
              onClick={() => onQuerySelect(query.query_text, query.datasource_slug)}
            >
          {/* Query preview */}
          <div className="flex items-start justify-between gap-2 mb-1">
            <div className="flex-1 min-w-0">
              <div className="text-xs font-mono text-foreground truncate">
                {getQueryPreview(query.query_text)}
              </div>
            </div>
            <div className="flex items-center gap-1 flex-shrink-0">
              {/* Star button */}
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={(e) => handleToggleSaved(query.query_id, query.is_saved, e)}
                    className={`p-1 rounded hover:bg-muted transition-colors ${
                      query.is_saved ? 'text-primary' : 'text-muted-foreground'
                    }`}
                    aria-label={query.is_saved ? 'Unsave query' : 'Save query'}
                  >
                    <svg className="w-3 h-3" fill={query.is_saved ? 'currentColor' : 'none'} stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                    </svg>
                  </button>
                </TooltipTrigger>
                <TooltipContent>{query.is_saved ? 'Unsave query' : 'Save query'}</TooltipContent>
              </Tooltip>
              {/* Delete button */}
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={(e) => handleDeleteQuery(query.query_id, e)}
                    className="p-1 rounded hover:bg-error/10 text-muted-foreground hover:text-error-foreground transition-colors"
                    aria-label="Delete query"
                  >
                    <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                    </svg>
                  </button>
                </TooltipTrigger>
                <TooltipContent>Delete query</TooltipContent>
              </Tooltip>
            </div>
          </div>

          {/* Metadata */}
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            {/* Timestamp */}
            <span>{formatTimestamp(query.executed_at)}</span>

            {/* Status indicator */}
            {query.status === 'success' ? (
              <span className="text-success-foreground">✓</span>
            ) : (
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className="text-error-foreground cursor-help">✗</span>
                </TooltipTrigger>
                <TooltipContent>{query.error_message || 'Error'}</TooltipContent>
              </Tooltip>
            )}

            {/* Execution time */}
            {query.execution_time_ms && (
              <span>{formatExecutionTime(query.execution_time_ms)}</span>
            )}

            {/* Row count */}
            {query.row_count !== null && query.row_count !== undefined && (
              <span>{query.row_count.toLocaleString()} rows</span>
            )}
          </div>
            </div>
          </TooltipTrigger>
          <TooltipContent>
            <div className="font-mono text-xs max-w-md">{query.query_text}</div>
          </TooltipContent>
        </Tooltip>
        ))}

        {/* Loading more indicator */}
        {isLoadingMore && (
          <div className="flex items-center justify-center py-4">
            <Spinner size="sm" className="text-primary" />
            <span className="ml-2 text-xs text-muted-foreground">Loading more...</span>
          </div>
        )}

        {/* Infinite scroll sentinel */}
        {hasMore && !isLoadingMore && (
          <div ref={observerTarget} className="h-4" />
        )}

        {/* End of list indicator */}
        {!hasMore && queryHistory.length > 0 && (
          <div className="py-3 text-center text-xs text-muted-foreground">
            End of query history
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="flex flex-col h-full">
      {/* Saved filter toggle - always visible */}
      <div className="px-3 py-2 border-b border-border bg-muted">
        <label className="flex items-center gap-2 text-xs cursor-pointer">
          <input
            type="checkbox"
            checked={showSavedOnly}
            onChange={(e) => setShowSavedOnly(e.target.checked)}
            className="rounded border-input text-primary focus:ring-ring"
          />
          <span className="text-foreground">Saved only</span>
        </label>
      </div>

      {/* Content area - loading/error/empty/list */}
      {renderContent()}

      {/* Confirm Dialog */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
    </div>
  );
};

export default QueryHistorySidebar;
