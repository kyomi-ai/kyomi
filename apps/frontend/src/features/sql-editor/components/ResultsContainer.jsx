// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ResultsContainer Component
 *
 * Main orchestrator for tabbed results display.
 * Integrates TabBar, ResultsTable, ResultsError, ResultsLoading, and ChartBuilderModal.
 * Charts are created via modal for adding to dashboards (not displayed in SQL editor).
 */

import { useState, useCallback, useMemo, memo, useDeferredValue, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { useSqlEditorStore, useActiveTab, useTableUIState } from '../store';
import TabBar from './TabBar';
import ResultsTable from './ResultsTable';
import ResultsError from './ResultsError';
import ResultsLoading from './ResultsLoading';
import ChartBuilderModal from '../../../components/ChartBuilderModal';
import SaveDashboardModal from '../../../components/SaveDashboardModal';
import apiClient from '../../../api/apiClient';
import { queryService } from '../../../services/queryService';
import * as yaml from 'js-yaml';
import useRenderLogger from '../../../hooks/useRenderLogger';
import { toast } from '../../../lib/toast';

/**
 * ResultsContainer component
 *
 * This is the main component that displays all query results in tabs.
 * Uses Zustand store for state management.
 *
 * Usage:
 * ```jsx
 * <ResultsContainer editorRef={editorRef} />
 * ```
 */
const ResultsContainer = ({ editorRef, hideCreateChartButton = false, onDatasourceChange = null, onRunQuery = null }) => {
  const navigate = useNavigate();

  // Use selective subscriptions to avoid re-rendering on unrelated store changes
  // IMPORTANT: Use shallow equality for tabs to prevent re-renders when tab content changes
  const tabs = useSqlEditorStore((state) => state.tabs, (a, b) => {
    // Only re-render if tabs array length/identity changes, not content
    return a.length === b.length && a.every((tab, i) => tab.id === b[i].id);
  });
  const activeTabId = useSqlEditorStore((state) => state.activeTabId);
  const setActiveTab = useSqlEditorStore((state) => state.setActiveTab);
  const removeTab = useSqlEditorStore((state) => state.removeTab);
  const updateTab = useSqlEditorStore((state) => state.updateTab);
  const togglePin = useSqlEditorStore((state) => state.togglePin);
  const setTableUIState = useSqlEditorStore((state) => state.setTableUIState);
  const setDefaultPageSize = useSqlEditorStore((state) => state.setDefaultPageSize);

  const activeTab = useActiveTab();
  const tableUIState = useTableUIState(activeTabId);

  // Defer table data updates to keep tab switching responsive
  // The tab highlight changes immediately, table updates in background
  const deferredActiveTab = useDeferredValue(activeTab);
  const isPending = activeTab !== deferredActiveTab;

  // Development: Log re-renders
  useRenderLogger('ResultsContainer', {
    tabCount: tabs.length,
    activeTabId,
    activeTabStatus: activeTab?.status
  });

  const [isGeneratingChart, setIsGeneratingChart] = useState(false);
  const [isPaginating, setIsPaginating] = useState(false);
  const [showChartBuilder, setShowChartBuilder] = useState(false);
  const [generatedChartML, setGeneratedChartML] = useState(null);
  const [saveDashboardModal, setSaveDashboardModal] = useState({ isOpen: false, chartML: null });

  // Handle tab click - update activeTab immediately, useDeferredValue handles the deferred table render
  // NOTE: Clicking a tab just switches which results are visible - it does NOT change the datasource selector.
  // Datasource only changes when user explicitly restores SQL from a tab or selects from history.
  const handleTabClick = useCallback((tabId) => {
    setActiveTab(tabId);
  }, [setActiveTab]);

  // Handle tab close
  const handleTabClose = useCallback((tabId) => {
    removeTab(tabId);
  }, [removeTab]);

  // Handle pin toggle
  const handleTogglePin = useCallback((tabId) => {
    togglePin(tabId);
  }, [togglePin]);

  // Mark tab as expired (results not restorable)
  const markTabExpired = useCallback((errorMessage = 'Query results expired. Please re-run the query.') => {
    updateTab(activeTabId, {
      needsRefresh: false,
      status: 'idle',
      result: undefined,
      error: { message: errorMessage },
    });
  }, [activeTabId, updateTab]);

  // Auto-refresh tabs that were restored from localStorage (needsRefresh flag)
  useEffect(() => {
    if (!activeTab || !activeTab.needsRefresh) {
      return;
    }

    // No queryHandle means we can't refresh (missing pagination info)
    if (!activeTab.result?.queryHandle) {
      markTabExpired();
      return;
    }

    const refreshTab = async () => {
      const pageSize = tableUIState.pageSize || 50;

      try {
        setIsPaginating(true);

        // Re-fetch first page using queryHandle (uses queryService)
        const results = await queryService.fetchPage(activeTab.result.queryHandle, 1, pageSize);

        // Update tab with fresh data
        updateTab(activeTabId, {
          result: {
            columns: results.columns || activeTab.result.columns,
            rows: results.rows || [],
            rowCount: results.rows?.length || 0,
            totalRows: activeTab.result.totalRows,
            queryHandle: activeTab.result.queryHandle, // Preserve queryHandle
            executionTime: activeTab.result.executionTime,
            bytesProcessed: activeTab.result.bytesProcessed,
          },
          needsRefresh: false,
        });

      } catch (error) {

        // Check if it's a job expiry error (e.g., BigQuery caches results for 24 hours)
        const isExpired = error.message && (
          error.message.includes('Not found') ||
          error.message.includes('404') ||
          error.message.includes('not found') ||
          error.message.includes('expired')
        );

        markTabExpired(
          isExpired
            ? 'Query results expired. Please re-run the query.'
            : 'Failed to restore results. Please re-run the query.'
        );
      } finally {
        setIsPaginating(false);
      }
    };

    refreshTab();
  }, [activeTab, activeTabId, updateTab, tableUIState.pageSize, markTabExpired]);

  // Handle page change - uses queryService.fetchPage (works for all datasources)
  const handlePageChange = useCallback(async (page) => {
    if (!activeTabId || !activeTab) return;

    // Get queryHandle from current result (set on initial query execution)
    const queryHandle = activeTab.result?.queryHandle;

    if (!queryHandle) {
      return;
    }

    try {
      // Set pagination loading state (keeps table visible with overlay)
      setIsPaginating(true);

      // Fetch page using queryHandle (supports all datasource types!)
      const pageSize = tableUIState.pageSize || 50;
      const results = await queryService.fetchPage(queryHandle, page, pageSize);

      // Update tab with new page of results
      updateTab(activeTabId, {
        status: 'success',
        result: {
          columns: results.columns || activeTab.result.columns, // Preserve columns from first page
          rows: results.rows || [],
          rowCount: results.rows?.length || 0,
          totalRows: activeTab.result.totalRows, // Preserve total from first page
          queryHandle: activeTab.result.queryHandle, // Preserve queryHandle
          executionTime: activeTab.result.executionTime, // Preserve execution time
          bytesProcessed: activeTab.result.bytesProcessed, // Preserve bytes processed
        },
      });

      // Update current page in UI state
      setTableUIState(activeTabId, { currentPage: page });

      // Wait for next frame to ensure data renders before clearing loading state
      requestAnimationFrame(() => {
        setIsPaginating(false);
      });
    } catch (error) {

      // Determine error message
      let errorMessage = error.response?.data?.detail || error.message || 'Failed to fetch page';

      // Check if results expired
      if (error.message && (
        error.message.includes('Not found') ||
        error.message.includes('404') ||
        error.message.includes('not found') ||
        error.message.includes('expired')
      )) {
        errorMessage = 'Query results expired. Please re-run the query.';
      }

      // Update tab with error
      updateTab(activeTabId, {
        status: 'error',
        error: {
          message: errorMessage,
        },
      });

      // Also wait for next frame on error
      requestAnimationFrame(() => {
        setIsPaginating(false);
      });
    }
  }, [activeTabId, activeTab, tableUIState, updateTab, setTableUIState]);

  // Handle page size change - re-execute query with new page size (uses queryService)
  const handlePageSizeChange = useCallback(async (newPageSize) => {
    if (!activeTabId || !activeTab) return;

    // Get queryHandle from current result to get datasource info
    const queryHandle = activeTab.result?.queryHandle;
    if (!queryHandle) {
      return;
    }

    try {
      // Set pagination loading state
      setIsPaginating(true);

      // Save as user's default preference for future tabs
      setDefaultPageSize(newPageSize);

      // Re-execute query with new page size (uses queryService)
      const results = await queryService.executeQuery(queryHandle.sql, {
        slug: queryHandle.datasourceSlug,
        type: queryHandle.datasourceType,
      }, {
        pageSize: newPageSize,
      });

      // Update tab with new results
      updateTab(activeTabId, {
        status: 'success',
        result: {
          columns: results.columns || [],
          rows: results.rows || [],
          rowCount: results.rows?.length || 0,
          totalRows: results.totalRows,
          queryHandle: results.queryHandle, // New queryHandle from re-execution
          executionTime: results.executionTimeMs,
          bytesProcessed: results.bytesProcessed,
        },
      });

      // Update page size and reset to page 1
      setTableUIState(activeTabId, { pageSize: newPageSize, currentPage: 1 });
    } catch (error) {

      updateTab(activeTabId, {
        status: 'error',
        error: {
          message: error.response?.data?.detail || error.message || 'Failed to change page size',
        },
      });
    } finally {
      setIsPaginating(false);
    }
  }, [activeTabId, activeTab, updateTab, setTableUIState, setDefaultPageSize]);

  // Sorting disabled for v1.0 - requires server-side implementation
  // const handleSortChange = useCallback((column, isCtrlClick) => {
  //   // Sorting logic removed - not implemented for v1.0
  // }, [activeTabId, tableUIState, setTableUIState]);

  // Handle chart creation - open chart builder modal with generated chart
  const handleCreateChart = useCallback(async () => {
    if (!activeTab || !activeTab.result) return;

    try {
      setIsGeneratingChart(true);

      // Extract column names (columns can be string[] or ColumnMetadata[])
      const columnNames = activeTab.result.columns.map(col =>
        typeof col === 'string' ? col : col.name
      );

      const response = await apiClient.post('/api/v1/chart/generate', {
        sql_text: activeTab.query,
        columns: columnNames,
        rows: activeTab.result.rows.slice(0, 100),
        user_context: null,
        datasource_slug: activeTab.datasourceSlug || null,
        datasource_type: activeTab.datasourceType || null,
      });

      const chartML = yaml.load(response.data.chart_yaml);

      // Open chart builder modal with generated chart
      setGeneratedChartML(chartML);
      setShowChartBuilder(true);
    } catch (err) {
      toast.error('Failed to generate chart: ' + (err.response?.data?.detail || err.message));
    } finally {
      setIsGeneratingChart(false);
    }
  }, [activeTab]);

  // Handle chart save from modal - open dashboard save modal
  const handleSaveChart = useCallback((chartML) => {
    setShowChartBuilder(false);
    setGeneratedChartML(null);
    // Wrap ChartML in markdown code blocks for dashboard
    const chartMarkdown = `\`\`\`chartml\n${yaml.dump(chartML)}\n\`\`\``;
    setSaveDashboardModal({ isOpen: true, chartML: chartMarkdown });
  }, []);

  // Handle saving chart to dashboard
  const handleSaveToDashboard = useCallback(async (mode, titleOrDashboardId, content) => {
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

      // Close modal after successful save
      setSaveDashboardModal({ isOpen: false, chartML: null });
    } catch (error) {
      throw error; // Let the modal handle the error
    }
  }, [navigate]);

  // Handle chart builder modal close
  const handleCloseChartBuilder = useCallback(() => {
    setShowChartBuilder(false);
    setGeneratedChartML(null);
  }, []);

  // Memoize Create Chart button to prevent unnecessary re-renders
  const createChartButton = useMemo(() => {
    if (hideCreateChartButton) return null;

    return (
      <button
        onClick={handleCreateChart}
        disabled={isGeneratingChart}
        className="px-2 md:px-3 py-1.5 text-xs font-medium bg-primary text-white rounded-md hover:bg-primary/90 transition-colors flex items-center gap-1 sm:gap-2 disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
      >
        {isGeneratingChart ? (
          <>
            <svg className="w-4 h-4 animate-spin flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
            </svg>
            <span className="hidden sm:inline">Generating...</span>
          </>
        ) : (
          <>
            <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
            </svg>
            <span className="hidden sm:inline">Create Chart</span>
          </>
        )}
      </button>
    );
  }, [hideCreateChartButton, handleCreateChart, isGeneratingChart]);

  // Handle re-running query for expired results
  const handleRerunQuery = useCallback(async () => {
    if (!activeTab || !activeTab.query || !onRunQuery) return;

    try {
      setIsPaginating(true); // Use pagination loading state

      // Track execution time on frontend
      const startTime = performance.now();

      // Update tab to running state
      updateTab(activeTabId, {
        status: 'running',
        error: undefined,
      });

      // Use the same query execution path as the initial run
      // onRunQuery uses queryService (unified interface)
      const results = await onRunQuery(activeTab.query);

      const frontendExecutionTime = performance.now() - startTime;

      // Update tab with fresh results
      updateTab(activeTabId, {
        status: 'success',
        result: {
          columns: results.columns || [],
          rows: results.rows || [],
          rowCount: results.rows?.length || 0,
          totalRows: results.totalRows,
          queryHandle: results.queryHandle,
          // Use backend executionTimeMs, fallback to frontend timing
          executionTime: results.executionTimeMs || frontendExecutionTime,
          bytesProcessed: results.bytesProcessed,
        },
        error: undefined,
        needsRefresh: false,
      });

      // Reset to page 1
      setTableUIState(activeTabId, { currentPage: 1 });
    } catch (error) {

      updateTab(activeTabId, {
        status: 'error',
        error: {
          message: error.response?.data?.detail || error.message || 'Failed to re-run query',
        },
      });
    } finally {
      setIsPaginating(false);
    }
  }, [activeTab, activeTabId, updateTab, onRunQuery, setTableUIState]);

  // Render content based on tab status - memoized to prevent unnecessary re-renders
  // Use deferredActiveTab for table rendering to keep UI responsive during tab switches
  const tabContent = useMemo(() => {
    if (!deferredActiveTab) {
      return (
        <div className="flex-1 flex items-center justify-center text-muted-foreground">
          <div className="text-center">
            <svg className="w-16 h-16 mx-auto mb-4 text-muted-foreground/50" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
            </svg>
            <p className="text-sm">No active result tab</p>
          </div>
        </div>
      );
    }

    // Show loading state for tabs that need refresh (restored from localStorage)
    if (deferredActiveTab.needsRefresh) {
      return <ResultsLoading message="Restoring query results..." />;
    }

    if (deferredActiveTab.status === 'running') {
      return <ResultsLoading message="Running query..." />;
    }

    // Show error if present (regardless of status - handles both 'error' and 'idle' with error)
    if (deferredActiveTab.error) {
      return (
        <ResultsError
          error={deferredActiveTab.error}
          onRerun={handleRerunQuery}
          rerunning={isPaginating}
        />
      );
    }

    if ((deferredActiveTab.status === 'success' || deferredActiveTab.status === 'streaming') && deferredActiveTab.result) {
      const isStreaming = deferredActiveTab.status === 'streaming';
      return (
        <div className="flex-1 flex flex-col min-h-0 relative">
          {/* Loading overlay during deferred updates */}
          {isPending && (
            <div className="absolute inset-0 bg-card/50 z-10 pointer-events-none" />
          )}

          {/* Streaming indicator */}
          {isStreaming && (
            <div className="flex items-center gap-2 px-3 py-1.5 bg-primary/5 border-b border-border text-xs text-muted-foreground">
              <div className="w-2 h-2 rounded-full bg-primary animate-pulse" />
              Streaming rows... ({deferredActiveTab.result.rowCount || 0} received)
            </div>
          )}

          {/* Results table */}
          <div className="flex-1 min-h-0">
            <ResultsTable
              result={deferredActiveTab.result}
              sortBy={[]}
              onSortChange={undefined}
              currentPage={tableUIState.currentPage}
              onPageChange={handlePageChange}
              pageSize={tableUIState.pageSize}
              onPageSizeChange={handlePageSizeChange}
              isPaginating={isPaginating}
              headerActions={!isStreaming ? createChartButton : undefined}
            />
          </div>
        </div>
      );
    }

    return null;
  }, [
    deferredActiveTab,
    isPending,
    tableUIState,
    isPaginating,
    handleRerunQuery,
    handlePageChange,
    handlePageSizeChange,
    createChartButton,
  ]);

  if (tabs.length === 0) {
    return null; // Don't show anything when there are no tabs
  }

  return (
    <>
      <div className="flex-1 flex flex-col min-h-0 border border-input rounded-md overflow-hidden bg-card">
        <TabBar
          tabs={tabs}
          activeTabId={activeTabId}
          onTabClick={handleTabClick}
          onTabClose={handleTabClose}
          onTogglePin={handleTogglePin}
          editorRef={editorRef}
          onDatasourceChange={onDatasourceChange}
        />
        {tabContent}
      </div>

      {/* Chart Builder Modal */}
      {showChartBuilder && generatedChartML && (
        <ChartBuilderModal
          chartML={generatedChartML}
          onSave={handleSaveChart}
          onClose={handleCloseChartBuilder}
          datasourceSlug={activeTab?.datasourceSlug}
          datasourceType={activeTab?.datasourceType}
        />
      )}

      {/* Save to Dashboard Modal */}
      <SaveDashboardModal
        isOpen={saveDashboardModal.isOpen}
        onClose={() => setSaveDashboardModal({ isOpen: false, chartML: null })}
        onSave={handleSaveToDashboard}
        messageContent={saveDashboardModal.chartML}
        apiClient={apiClient}
      />
    </>
  );
};

export default memo(ResultsContainer);
