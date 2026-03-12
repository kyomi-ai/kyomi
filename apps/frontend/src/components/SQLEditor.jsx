// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useRef, useEffect, useCallback, useMemo } from 'react';
import MonacoSQLEditor from './MonacoSQLEditor';
import DatasourceCatalogTree from './DatasourceCatalogTree';
import QueryHistorySidebar from './QueryHistorySidebar';
import { useTheme } from '../context/ThemeContext';

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
import { ResultsContainer, useSqlEditorStore } from '../features/sql-editor';
import { useSQLDryRun } from '../hooks/useSQLDryRun';
import { useWebSocket } from '../context/WebSocketContext';
import apiClient from '../api/apiClient.js';
import useRenderLogger from '../hooks/useRenderLogger';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';
import { Spinner } from './ui/spinner';
import { CheckCircleIcon } from '@heroicons/react/24/solid';
import { ExclamationTriangleIcon } from '@heroicons/react/24/outline';
import Modal from './Modal';
import { formatSQLError } from '../features/sql-editor/utils/formatSQLError';

// Semantic search always enabled
const USE_SEMANTIC_SEARCH = true;

/**
 * SQLEditor - A reusable SQL editor component with query execution capabilities
 *
 * Features:
 * - Monaco SQL editor with syntax highlighting
 * - Cursor position indicator
 * - Dry run validation (optional)
 * - Query execution with loading state
 * - Results pane with resizable table
 * - Error display
 * - Keyboard shortcuts (Cmd/Ctrl+Enter to run)
 */
const SQLEditor = ({
  // Required query execution handler
  onRunQuery,

  // Optional copy button
  showCopyButton = false,
  onCopy,

  // Optional placeholder for empty editor
  placeholder = 'Enter SQL query...',

  // Optional disable state
  disabled = false,

  // Optional controlled value (for embedding in modals)
  value,
  onChange,

  // Optional feature flags for embedding
  disableTabs = false,
  hideCreateChartButton = false,

  // Optional dry run state (passed from parent for modal embedding)
  dryRunning: externalDryRunning,
  dryRunResult: externalDryRunResult,

  // Optional existing results (for modal embedding)
  existingResults = null,

  // Mobile sidebar control (passed from SQLEditorPage)
  mobileSidebarOpen = false,
  onMobileSidebarClose,

  // Multi-datasource support
  datasourceSlug = null, // Datasource slug (e.g., "production-postgres")
  selectedDatasourceType = null, // null means unknown - skip dry run until known
  onDatasourceChange = null, // Callback when tab/history selection changes datasource: (slug) => void
}) => {
  const isMobile = useIsMobile();
  const { resolvedTheme } = useTheme();

  // Ref to Monaco editor for inserting text at cursor
  const editorRef = useRef(null);

  // State to track when editor is ready (for restoration timing)
  const [editorReady, setEditorReady] = useState(false);

  // Dry run validation hook (uses editorRef to avoid re-renders)
  // Use external dry run state if provided (for modal embedding), otherwise use internal
  // Pass datasource info for unified dry run across all datasource types
  const internalDryRun = useSQLDryRun(editorRef, true, {
    slug: datasourceSlug,
    type: selectedDatasourceType,
  });
  const dryRunning = externalDryRunning !== undefined ? externalDryRunning : internalDryRun.dryRunning;
  const dryRunResult = externalDryRunResult !== undefined ? externalDryRunResult : internalDryRun.dryRunResult;
  const triggerDryRun = internalDryRun.triggerDryRun;

  // WebSocket for streaming query results
  const { subscribe } = useWebSocket();

  // Zustand store for tabbed results - use selective subscriptions
  // CRITICAL: Only subscribe to tabs.length, not the full tabs array!
  // This prevents re-renders when tab content changes (during typing, results updates, etc.)
  const tabCount = useSqlEditorStore((state) => state.tabs.length);
  const addTab = useSqlEditorStore((state) => state.addTab);
  const updateTab = useSqlEditorStore((state) => state.updateTab);
  const queryText = useSqlEditorStore((state) => state.queryText);
  const setQueryText = useSqlEditorStore((state) => state.setQueryText);

  // Development: Log re-renders
  useRenderLogger('SQLEditor', { tabCount });

  const [queryRunning, setQueryRunning] = useState(false);
  const [editorPercentage, setEditorPercentage] = useState(50); // 50% by default
  const [isResizing, setIsResizing] = useState(false);
  const [resizeStartY, setResizeStartY] = useState(0);
  const [resizeStartPercentage, setResizeStartPercentage] = useState(50);
  const [copied, setCopied] = useState(false);
  const containerInternalRef = useRef(null);
  const topRowRef = useRef(null);

  // Catalog browser state (shared with RightSidebar)
  const [catalogSearchQuery, setCatalogSearchQuery] = useState('');
  const [refreshingCatalog, setRefreshingCatalog] = useState(false);
  const [catalogRefreshTrigger, setCatalogRefreshTrigger] = useState(0);

  // Query history state (shared with RightSidebar)
  const [historySearchQuery, setHistorySearchQuery] = useState('');
  const [historySearchInput, setHistorySearchInput] = useState(''); // Immediate input value
  const [historyRefreshTrigger, setHistoryRefreshTrigger] = useState(0);

  // Sidebar state - pixel-based width like Collections sidebar
  const [sidebarWidth, setSidebarWidth] = useState(320);
  const [isResizingSidebar, setIsResizingSidebar] = useState(false);
  const resizeSidebarStartX = useRef(0);
  const resizeSidebarStartWidth = useRef(320);

  // Active tab within the sidebar (catalog or history)
  const [activeTab, setActiveTab] = useState('catalog');

  // Debounce history search (300ms delay)
  useEffect(() => {
    const timer = setTimeout(() => {
      setHistorySearchQuery(historySearchInput);
    }, 300);

    return () => clearTimeout(timer);
  }, [historySearchInput]);

  // Load persisted query text from store when available
  // This runs when either the store rehydrates OR the editor mounts (whichever happens second)
  useEffect(() => {

    if (queryText && editorReady && editorRef.current) {
      // Only restore if the editor is currently empty (avoid overwriting user edits)
      const currentValue = editorRef.current.getValue();
      if (!currentValue || currentValue.trim() === '') {
        editorRef.current.setValue(queryText);
      } else {
      }
    } else {
    }
  }, [queryText, editorReady]); // Re-run when store rehydrates or editor becomes ready

  // Callback when Monaco editor mounts
  const handleEditorMount = useCallback(() => {
    setEditorReady(true);
  }, []);

  // Re-trigger dry run when datasource type changes
  useEffect(() => {
    triggerDryRun();
  }, [selectedDatasourceType, triggerDryRun]);

  // onChange handler - triggers dry run (no state update, no re-render)
  const handleEditorChange = useCallback(() => {
    triggerDryRun();

    // Save current query to store for persistence
    const currentValue = editorRef.current?.getValue() || '';
    setQueryText(currentValue);

    // If parent provided onChange (controlled mode), call it with current value
    if (onChange) {
      onChange(currentValue);
    }
  }, [triggerDryRun, onChange, setQueryText]);

  // Table details state (for when user clicks info button)
  const [selectedTable, setSelectedTable] = useState(null);
  const [tableDetails, setTableDetails] = useState(null);
  const [loadingTableDetails, setLoadingTableDetails] = useState(false);

  // Handle table click from tree - insert into editor
  const handleTableClick = useCallback((tableId) => {
    const currentValue = editorRef.current?.getValue() || '';
    // Format table name based on datasource type
    let formattedTable;
    if (selectedDatasourceType === 'bigquery') {
      // BigQuery uses backticks for table identifiers
      formattedTable = `\`${tableId}\``;
    } else {
      // PostgreSQL, ClickHouse, etc. use plain identifiers
      formattedTable = tableId;
    }
    const newValue = currentValue + (currentValue ? ' ' : '') + formattedTable;
    editorRef.current?.setValue(newValue);
    setQueryText(newValue); // Persist to store
  }, [setQueryText, selectedDatasourceType]);

  // Handle column click from tree - insert into editor with comma and newline
  const handleColumnClick = useCallback((columnName) => {
    // Use editor ref to insert at cursor position
    if (editorRef.current) {
      editorRef.current.insertTextAtCursor(`${columnName},\n`);
    }
  }, [editorRef]);

  // Handle query history selection - load query into editor and restore datasource
  const handleQuerySelect = useCallback((selectedQueryText, datasourceSlug) => {
    editorRef.current?.setValue(selectedQueryText);
    setQueryText(selectedQueryText); // Persist to store
    // Restore datasource selection if provided
    if (datasourceSlug && onDatasourceChange) {
      onDatasourceChange(datasourceSlug);
    }
  }, [setQueryText, onDatasourceChange]);

  // Handle catalog refresh
  const handleRefreshCatalog = useCallback(async () => {
    if (!datasourceSlug) return;
    setRefreshingCatalog(true);

    try {
      const response = await apiClient.refreshCatalog(datasourceSlug, { force: false });

      if (response.status === 'started' || response.status === 'completed') {
        // Trigger catalog reload by incrementing the refresh trigger
        setCatalogRefreshTrigger(prev => prev + 1);
      }
      // All status messages (rate limited, already running, etc.) are shown in the status bar
    } catch (error) {
    } finally {
      setRefreshingCatalog(false);
    }
  }, [datasourceSlug, setCatalogRefreshTrigger]);

  // Handle table details request from tree
  const handleTableDetails = useCallback(async (table) => {
    setSelectedTable(table);
    setLoadingTableDetails(true);
    setTableDetails(null);

    try {
      const response = await apiClient.getTableInfo({
        table_id: table.full_table_id
      });
      setTableDetails(response);
    } catch (error) {
      setTableDetails({
        status: 'error',
        error: error.response?.data?.detail || error.message
      });
    } finally {
      setLoadingTableDetails(false);
    }
  }, []);

  // Insert table name from details view
  const handleInsertTableFromDetails = useCallback((tableId) => {
    const currentValue = editorRef.current?.getValue() || '';
    // Format table name based on datasource type
    let formattedTable;
    if (selectedDatasourceType === 'bigquery') {
      // BigQuery uses backticks for table identifiers
      formattedTable = `\`${tableId}\``;
    } else {
      // PostgreSQL, ClickHouse, etc. use plain identifiers
      formattedTable = tableId;
    }
    const newValue = currentValue + (currentValue ? ' ' : '') + formattedTable;
    editorRef.current?.setValue(newValue);
    setQueryText(newValue); // Persist to store
    setSelectedTable(null);
    setTableDetails(null);
  }, [setQueryText, selectedDatasourceType]);



  // Start a streaming query — sets up WS subscriptions that progressively update the tab.
  // Returns true if streaming was initiated, false if not available.
  const startStreamingQuery = useCallback(async (sql, tabId) => {
    const accumulatedRows = [];
    let streamColumns = [];

    // Generate request ID client-side so WS subscriptions can filter
    // immediately — avoids a race where the error WS message arrives
    // before the HTTP response sets the request ID.
    const requestId = crypto.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`;

    // Subscribe to stream events BEFORE making the HTTP request
    const unsubs = [];

    const filterMsg = (msg, handler) => {
      if (msg.data?.request_id === requestId) {
        handler(msg);
      }
    };

    const cleanupSubs = () => unsubs.forEach(u => u());

    unsubs.push(subscribe('query_stream_header', (msg) => filterMsg(msg, (m) => {
      streamColumns = m.data.columns || [];
      updateTab(tabId, {
        status: 'streaming',
        result: {
          columns: streamColumns,
          rows: [],
          rowCount: 0,
          totalRows: m.data.total_rows ?? null,
        },
      });
    })));

    unsubs.push(subscribe('query_stream_chunk', (msg) => filterMsg(msg, (m) => {
      const chunkRows = m.data.rows || [];
      accumulatedRows.push(...chunkRows);
      updateTab(tabId, {
        status: 'streaming',
        result: {
          columns: streamColumns,
          rows: [...accumulatedRows],
          rowCount: accumulatedRows.length,
        },
      });
    })));

    unsubs.push(subscribe('query_stream_complete', (msg) => filterMsg(msg, (m) => {
      updateTab(tabId, {
        status: 'success',
        result: {
          columns: streamColumns,
          rows: [...accumulatedRows],
          rowCount: accumulatedRows.length,
          totalRows: m.data.total_rows_returned ?? accumulatedRows.length,
          executionTime: m.data.execution_time_ms,
          bytesProcessed: m.data.bytes_processed,
        },
      });
      cleanupSubs();
      setQueryRunning(false);
      setHistoryRefreshTrigger(prev => prev + 1);
    })));

    unsubs.push(subscribe('query_stream_error', (msg) => filterMsg(msg, (m) => {
      updateTab(tabId, {
        status: 'error',
        error: { message: m.data.error || 'Stream error' },
      });
      cleanupSubs();
      setQueryRunning(false);
      setHistoryRefreshTrigger(prev => prev + 1);
    })));

    // Start the stream — pass the client-generated request_id
    await apiClient.post('/api/v1/datasources/query/stream', {
      sql,
      datasource: datasourceSlug,
      limit: 10000,
      offset: 0,
      include_total: true,
      request_id: requestId,
    });
  }, [subscribe, updateTab, datasourceSlug]);

  // Wrap onRunQuery with error handling and state management
  const handleRunQuery = useCallback(async () => {
    setQueryRunning(true);

    // Track execution time on frontend
    const startTime = performance.now();

    // Get selected SQL or full editor content if no selection
    const currentSQL = editorRef.current?.getSelectedOrFullText() || '';

    // Create a new tab in running state
    const tabId = addTab({
      label: 'Query', // Simple label - tabs use colors to differentiate
      query: currentSQL,
      status: 'running',
      datasourceSlug: datasourceSlug,
      datasourceType: selectedDatasourceType,
    });

    // Use streaming for non-BigQuery datasources
    const useStreaming = selectedDatasourceType && selectedDatasourceType !== 'bigquery';

    if (useStreaming) {
      try {
        await startStreamingQuery(currentSQL, tabId);
        // Don't setQueryRunning(false) here — streaming callbacks handle it
        return;
      } catch (error) {
        const rawError = error.response?.data?.detail
          || error.response?.data?.error
          || error.message
          || 'Failed to start streaming query';
        updateTab(tabId, {
          status: 'error',
          error: { message: rawError },
        });
        setQueryRunning(false);
        setHistoryRefreshTrigger(prev => prev + 1);
        return;
      }
    }

    // Non-streaming path (BigQuery or fallback)
    try {
      // Pass current SQL text to the callback
      const results = await onRunQuery(currentSQL);
      const frontendExecutionTime = performance.now() - startTime;

      // Update tab with success results
      if (results) {
        updateTab(tabId, {
          status: 'success',
          result: {
            columns: results.columns || [],
            rows: results.rows || [],
            rowCount: results.rows?.length || 0,
            totalRows: results.totalRows, // For server-side pagination
            queryHandle: results.queryHandle, // For unified pagination (all datasource types)
            // Use backend executionTimeMs, fallback to frontend timing
            executionTime: results.executionTimeMs || frontendExecutionTime,
            bytesProcessed: results.bytesProcessed,
          },
        });
      }

      // Trigger history sidebar refresh after successful query
      setHistoryRefreshTrigger(prev => prev + 1);
    } catch (error) {

      // Extract meaningful error message from API response
      const rawError = error.response?.data?.detail
        || error.response?.data?.message
        || error.response?.data?.error
        || error.message
        || 'An unknown error occurred';

      // Update tab with error
      updateTab(tabId, {
        status: 'error',
        error: {
          message: rawError,
        },
      });

      // Trigger history sidebar refresh even on error (error queries are saved too)
      setHistoryRefreshTrigger(prev => prev + 1);
    } finally {
      setQueryRunning(false);
    }
  }, [onRunQuery, addTab, updateTab, startStreamingQuery, selectedDatasourceType]);

  const handleCopy = useCallback(() => {
    if (onCopy) {
      onCopy();
    } else {
      const currentValue = editorRef.current?.getValue() || '';
      navigator.clipboard.writeText(currentValue);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  }, [onCopy]);


  const handleResizeStart = useCallback((e) => {
    setResizeStartY(e.clientY);
    setResizeStartPercentage(editorPercentage);
    setIsResizing(true);
    e.preventDefault();
  }, [editorPercentage]);

  useEffect(() => {
    if (!isResizing || !containerInternalRef.current) return;

    const containerHeight = containerInternalRef.current.getBoundingClientRect().height;

    const handleMouseMove = (e) => {
      const diff = e.clientY - resizeStartY;
      const percentageChange = (diff / containerHeight) * 100;
      const newPercentage = resizeStartPercentage + percentageChange;

      // Clamp between 20% and 80%
      setEditorPercentage(Math.max(20, Math.min(newPercentage, 80)));
    };

    const handleMouseUp = () => {
      setIsResizing(false);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };

    document.body.style.cursor = 'row-resize';
    document.body.style.userSelect = 'none';

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);

    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
  }, [isResizing, resizeStartY, resizeStartPercentage]);

  // Sidebar resize handlers (pixel-based like Collections)
  const handleSidebarResizeStart = useCallback((e) => {
    resizeSidebarStartX.current = e.clientX;
    resizeSidebarStartWidth.current = sidebarWidth;
    setIsResizingSidebar(true);
    e.preventDefault();
  }, [sidebarWidth]);

  useEffect(() => {
    if (!isResizingSidebar) return;

    const handleMouseMove = (e) => {
      // Moving left increases width (sidebar is on right)
      const diff = resizeSidebarStartX.current - e.clientX;
      const newWidth = resizeSidebarStartWidth.current + diff;
      // Clamp between 280px and 480px
      setSidebarWidth(Math.max(280, Math.min(newWidth, 480)));
    };

    const handleMouseUp = () => {
      setIsResizingSidebar(false);
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
  }, [isResizingSidebar]);

  // Memoize catalog content to prevent re-rendering on every state change
  const memoizedCatalogContent = useMemo(() => (
    <div className="flex flex-col h-full">
      {/* Catalog Search */}
      <div className="px-3 py-2 border-b border-border">
        <div className="relative">
          <input
            type="text"
            placeholder="Search tables..."
            value={catalogSearchQuery}
            onChange={(e) => setCatalogSearchQuery(e.target.value)}
            className="w-full px-3 py-2 pr-8 text-sm border border-border rounded-md focus:outline-none focus:ring-2 focus:ring-ring bg-background text-foreground"
          />
          {catalogSearchQuery && (
            <button
              onClick={() => setCatalogSearchQuery('')}
              className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground transition-colors p-0.5"
              aria-label="Clear search"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          )}
        </div>
      </div>

      {/* Catalog Tree */}
      <div className="flex-1 overflow-auto">
        <DatasourceCatalogTree
          onTableClick={handleTableClick}
          onColumnClick={handleColumnClick}
          onTableDetails={handleTableDetails}
          searchQuery={catalogSearchQuery}
          useSemanticSearch={USE_SEMANTIC_SEARCH}
          refreshTrigger={catalogRefreshTrigger}
          datasourceId={datasourceSlug}
        />
      </div>
    </div>
  ), [catalogSearchQuery, handleTableClick, handleColumnClick, handleTableDetails, catalogRefreshTrigger, datasourceSlug]);

  // Memoize history content to prevent re-rendering on every state change
  const memoizedHistoryContent = useMemo(() => (
    <div className="flex flex-col h-full">
      {/* History Search */}
      <div className="px-3 py-2 border-b border-border flex-shrink-0">
        <div className="relative">
          <input
            type="text"
            placeholder="Search query history..."
            value={historySearchInput}
            onChange={(e) => setHistorySearchInput(e.target.value)}
            className="w-full px-3 py-2 pr-8 text-sm border border-border rounded-md focus:outline-none focus:ring-2 focus:ring-ring bg-background text-foreground"
          />
          {historySearchInput && (
            <button
              onClick={() => setHistorySearchInput('')}
              className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground transition-colors p-0.5"
              aria-label="Clear search"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          )}
        </div>
      </div>

      <div className="flex-1 min-h-0">
        <QueryHistorySidebar
          onQuerySelect={handleQuerySelect}
          searchQuery={historySearchQuery}
          onSearchChange={setHistorySearchInput}
          refreshTrigger={historyRefreshTrigger}
        />
      </div>
    </div>
  ), [historySearchInput, historySearchQuery, handleQuerySelect, historyRefreshTrigger]);

  return (
    <div ref={containerInternalRef} className="flex flex-col h-full w-full gap-2 overflow-hidden">
      {/* Top Row: Editor + Right Sidebar */}
      <div
        ref={topRowRef}
        className={`flex ${!isResizing ? 'transition-all duration-300 ease-in-out' : ''}`}
        style={{
          height: tabCount > 0 ? `${editorPercentage}%` : '100%',
          minHeight: '150px',
          flexShrink: 0
        }}
      >
        {/* Editor + Sidebar Container */}
        <div className="relative border-l border-r border-b border-border rounded-b-md overflow-hidden flex flex-1 min-w-0">
          {/* Editor Section */}
          <div className="relative flex flex-col flex-1 min-w-0">

        {/* Top-right buttons */}
        <div className="absolute top-2 right-2 z-20 flex gap-2">
          {showCopyButton && (
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={handleCopy}
                  className="p-2 bg-card hover:bg-accent text-foreground border border-border rounded-md transition-colors shadow-sm"
                  aria-label="Copy SQL"
                >
                  {copied ? (
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                    </svg>
                  ) : (
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
                    </svg>
                  )}
                </button>
              </TooltipTrigger>
              <TooltipContent>Copy SQL</TooltipContent>
            </Tooltip>
          )}

        </div>

        <div className="flex flex-col h-full">
          {/* Monaco SQL Editor */}
          <div className="flex-1 min-h-0 overflow-hidden">
            <MonacoSQLEditor
              ref={editorRef}
              value={value !== undefined ? value : ""}
              onChange={handleEditorChange}
              onMount={handleEditorMount}
              onRunQuery={handleRunQuery}
              placeholder={placeholder}
              disabled={disabled}
              fontSize={12}
              editorTheme={resolvedTheme}
            />
          </div>

          {/* Query Status and Run Button */}
          <div className="px-4 py-2 border-t border-border bg-muted flex-shrink-0 overflow-hidden flex items-center justify-between">
            <div className="flex-1 min-w-0">
              {dryRunning && (
                <div className="flex items-center gap-2 text-xs text-muted-foreground">
                  <Spinner size="sm" className="text-muted-foreground" />
                  <span>Validating query...</span>
                </div>
              )}

              {!dryRunning && dryRunResult && (
                <div className="text-xs" style={{ minHeight: '20px' }}>
                  {dryRunResult.valid ? (
                    <div className="flex items-center gap-2">
                      <CheckCircleIcon className="h-5 w-5 text-success-foreground" />
                      <span className="text-muted-foreground">{dryRunResult.message}</span>
                    </div>
                  ) : (
                    (() => {
                      const message = dryRunResult.message || '';
                      const isAuthError = message.toLowerCase().includes('authentication') ||
                                         message.toLowerCase().includes('unauthorized') ||
                                         message.toLowerCase().includes('credentials') ||
                                         message.toLowerCase().includes('oauth') ||
                                         message.toLowerCase().includes('401');

                      if (isAuthError) {
                        return (
                          <div className="flex items-center gap-2">
                            <ExclamationTriangleIcon className="h-5 w-5 text-warning-foreground flex-shrink-0" />
                            <span className="text-warning-foreground text-xs truncate">
                              Authentication required - check datasource credentials in Settings
                            </span>
                          </div>
                        );
                      }

                      return (
                        <div className="flex items-center gap-2">
                          <ExclamationTriangleIcon className="h-5 w-5 text-error-foreground flex-shrink-0" />
                          <span className="text-error-foreground text-xs truncate">
                            {formatSQLError(message)}
                          </span>
                        </div>
                      );
                    })()
                  )}
                </div>
              )}
            </div>
            <button
              onClick={handleRunQuery}
              disabled={queryRunning || disabled}
              className="px-3 py-1.5 text-xs font-medium bg-primary text-white hover:bg-primary/90 rounded-md transition-colors disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
            >
              {queryRunning ? 'Running...' : 'Run Query'}
            </button>
          </div>
        </div>
          </div>
          {/* End Editor Section */}

        {/* Right Sidebar - render based on mobile/desktop */}
        {mobileSidebarOpen && (
          isMobile ? (
            // Mobile: Fixed overlay with backdrop
            // 4rem main header + 3.5rem (sm:4rem) SQL Editor header
            <>
              <div
                className="fixed top-[7.5rem] sm:top-[8rem] left-0 right-0 bottom-0 bg-black/50 z-40"
                onClick={onMobileSidebarClose}
              />
              <div className="fixed top-[7.5rem] sm:top-[8rem] right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl">
                {/* Sidebar Header with tabs */}
                <div className="p-3 border-b border-border flex items-center justify-between flex-shrink-0">
                  <div className="flex items-center gap-1 bg-accent rounded-lg p-1">
                    <button
                      onClick={() => setActiveTab('catalog')}
                      className={`px-3 py-1.5 text-sm font-medium rounded-md transition-colors ${
                        activeTab === 'catalog'
                          ? 'bg-card text-foreground shadow-sm'
                          : 'text-muted-foreground hover:text-foreground'
                      }`}
                    >
                      Catalog
                    </button>
                    <button
                      onClick={() => setActiveTab('history')}
                      className={`px-3 py-1.5 text-sm font-medium rounded-md transition-colors ${
                        activeTab === 'history'
                          ? 'bg-card text-foreground shadow-sm'
                          : 'text-muted-foreground hover:text-foreground'
                      }`}
                    >
                      History
                    </button>
                  </div>
                  <div className="flex items-center gap-1">
                    {activeTab === 'catalog' && (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={handleRefreshCatalog}
                            disabled={refreshingCatalog}
                            className="p-1.5 text-muted-foreground hover:text-primary hover:bg-accent rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                            aria-label={refreshingCatalog ? "Refreshing catalog..." : "Refresh catalog"}
                          >
                            <svg
                              className={`w-4 h-4 ${refreshingCatalog ? 'animate-spin' : ''}`}
                              fill="none"
                              stroke="currentColor"
                              viewBox="0 0 24 24"
                            >
                              <path
                                strokeLinecap="round"
                                strokeLinejoin="round"
                                strokeWidth={2}
                                d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"
                              />
                            </svg>
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>{refreshingCatalog ? "Refreshing catalog..." : "Refresh catalog"}</TooltipContent>
                      </Tooltip>
                    )}
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          onClick={onMobileSidebarClose}
                          className="p-1 text-muted-foreground hover:text-foreground rounded transition-colors"
                          aria-label="Close"
                        >
                          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                          </svg>
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>Close</TooltipContent>
                    </Tooltip>
                  </div>
                </div>
                {/* Tab Content */}
                <div className="flex-1 overflow-hidden">
                  <div className={`h-full ${activeTab === 'catalog' ? 'block' : 'hidden'}`}>
                    {memoizedCatalogContent}
                  </div>
                  <div className={`h-full ${activeTab === 'history' ? 'block' : 'hidden'}`}>
                    {memoizedHistoryContent}
                  </div>
                </div>
              </div>
            </>
          ) : (
            // Desktop: Inline resizable sidebar
            <div
              className="border-l border-border bg-card flex h-full overflow-hidden flex-shrink-0"
              style={{ width: `${sidebarWidth}px` }}
            >
              {/* Resize Handle */}
              <Tooltip>
                <TooltipTrigger asChild>
                  <div
                    className="flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10"
                    onMouseDown={handleSidebarResizeStart}
                    aria-label="Drag to resize"
                  >
                    <div className="w-1 h-12 bg-border hover:bg-muted-foreground rounded transition-colors" />
                  </div>
                </TooltipTrigger>
                <TooltipContent>Drag to resize</TooltipContent>
              </Tooltip>

              {/* Main Content */}
              <div className="flex flex-col flex-1 min-w-0">
                {/* Sidebar Header with tabs */}
                <div className="p-3 border-b border-border flex items-center justify-between flex-shrink-0">
                  <div className="flex items-center gap-1 bg-accent rounded-lg p-1">
                    <button
                      onClick={() => setActiveTab('catalog')}
                      className={`px-3 py-1.5 text-sm font-medium rounded-md transition-colors ${
                        activeTab === 'catalog'
                          ? 'bg-card text-foreground shadow-sm'
                          : 'text-muted-foreground hover:text-foreground'
                      }`}
                    >
                      Catalog
                    </button>
                    <button
                      onClick={() => setActiveTab('history')}
                      className={`px-3 py-1.5 text-sm font-medium rounded-md transition-colors ${
                        activeTab === 'history'
                          ? 'bg-card text-foreground shadow-sm'
                          : 'text-muted-foreground hover:text-foreground'
                      }`}
                    >
                      History
                    </button>
                  </div>
                  <div className="flex items-center gap-1">
                    {activeTab === 'catalog' && (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={handleRefreshCatalog}
                            disabled={refreshingCatalog}
                            className="p-1.5 text-muted-foreground hover:text-primary hover:bg-accent rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                            aria-label={refreshingCatalog ? "Refreshing catalog..." : "Refresh catalog"}
                          >
                            <svg
                              className={`w-4 h-4 ${refreshingCatalog ? 'animate-spin' : ''}`}
                              fill="none"
                              stroke="currentColor"
                              viewBox="0 0 24 24"
                            >
                              <path
                                strokeLinecap="round"
                                strokeLinejoin="round"
                                strokeWidth={2}
                                d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"
                              />
                            </svg>
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>{refreshingCatalog ? "Refreshing catalog..." : "Refresh catalog"}</TooltipContent>
                      </Tooltip>
                    )}
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          onClick={onMobileSidebarClose}
                          className="p-1 text-muted-foreground hover:text-foreground rounded transition-colors"
                          aria-label="Close"
                        >
                          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                          </svg>
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>Close</TooltipContent>
                    </Tooltip>
                  </div>
                </div>

                {/* Tab Content */}
                <div className="flex-1 overflow-hidden">
                  <div className={`h-full ${activeTab === 'catalog' ? 'block' : 'hidden'}`}>
                    {memoizedCatalogContent}
                  </div>
                  <div className={`h-full ${activeTab === 'history' ? 'block' : 'hidden'}`}>
                    {memoizedHistoryContent}
                  </div>
                </div>
              </div>
            </div>
          )
        )}

        </div>
        {/* End Editor + Sidebar Container */}

      </div>
      {/* End Top Row */}

      {/* Resize Handle - show when there are tabs */}
      {tabCount > 0 && (
        <Tooltip>
          <TooltipTrigger asChild>
            <div
              className="flex items-center justify-center cursor-row-resize select-none py-1 -my-2 relative z-10"
              onMouseDown={handleResizeStart}
              aria-label="Drag to resize"
            >
              <div className="h-1 w-12 bg-border hover:bg-muted-foreground rounded transition-colors" />
            </div>
          </TooltipTrigger>
          <TooltipContent>Drag to resize</TooltipContent>
        </Tooltip>
      )}

      {/* Results Container - Tabbed results display */}
      {tabCount > 0 && (
        <div className={`flex-1 min-h-0 flex flex-col overflow-hidden ${!isResizing ? 'transition-all duration-300 ease-in-out' : ''}`}>
          <ResultsContainer editorRef={editorRef} hideCreateChartButton={hideCreateChartButton} onDatasourceChange={onDatasourceChange} onRunQuery={onRunQuery} />
        </div>
      )}

      {/* Table Details Modal */}
      <Modal
        show={!!selectedTable}
        onClose={() => {
          setSelectedTable(null);
          setTableDetails(null);
        }}
        title="Table Details"
        size="lg"
        footer={
          !loadingTableDetails && tableDetails && tableDetails.status === 'success' && (
            <button
              onClick={() => handleInsertTableFromDetails(selectedTable.full_table_id)}
              className="px-4 py-2 bg-primary text-white rounded-md hover:bg-primary/90 transition-colors"
            >
              Insert into Query
            </button>
          )
        }
      >
        {loadingTableDetails && (
          <div className="flex items-center justify-center py-8">
            <Spinner size="lg" className="text-primary" />
          </div>
        )}

        {!loadingTableDetails && tableDetails && tableDetails.status === 'error' && (
          <Alert variant="error">
            <AlertDescription>{tableDetails.error}</AlertDescription>
          </Alert>
        )}

        {!loadingTableDetails && tableDetails && tableDetails.status === 'success' && (
          <div className="space-y-4">
            {/* Table Header */}
            <div>
              <h4 className="font-mono text-lg text-foreground mb-2">
                {tableDetails.metadata.table?.replace(/`/g, '') || 'Unknown Table'}
              </h4>
              {tableDetails.metadata.desc && (
                <p className="text-sm text-foreground mb-3">
                  {tableDetails.metadata.desc}
                </p>
              )}
              <div className="flex items-center gap-4 text-sm text-muted-foreground">
                <span>
                  <strong>Rows:</strong> {tableDetails.metadata.rows?.toLocaleString() || 'Unknown'}
                </span>
              </div>
            </div>

            {/* Schema Table */}
            <div>
              <h5 className="text-sm font-semibold text-foreground mb-2">
                Schema ({tableDetails.metadata.cols?.length || 0} columns)
              </h5>
              <div className="border border-border rounded-md overflow-hidden">
                <table className="min-w-full divide-y divide-border">
                  <thead className="bg-muted">
                    <tr>
                      <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground uppercase">
                        Column
                      </th>
                      <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground uppercase">
                        Type
                      </th>
                      <th className="px-3 py-2 text-left text-xs font-medium text-muted-foreground uppercase">
                        Description
                      </th>
                    </tr>
                  </thead>
                  <tbody className="bg-card divide-y divide-border">
                    {(tableDetails.metadata.cols || []).map((field, index) => (
                      <tr key={index} className="hover:bg-accent">
                        <td className="px-3 py-2 text-sm font-mono text-foreground">
                          {field.name}
                        </td>
                        <td className="px-3 py-2 text-sm text-muted-foreground">
                          {field.type}
                        </td>
                        <td className="px-3 py-2 text-sm text-muted-foreground">
                          {field.desc || '-'}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          </div>
        )}
      </Modal>

    </div>
  );
};

export default SQLEditor;
