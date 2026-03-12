// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useCallback, useEffect, useState } from 'react';
import SQLEditor from '../components/SQLEditor';
import { queryService } from '../services/queryService';
import apiClient from '../api/apiClient';
import { useProductTour } from '../components/ProductTour';
import { Tooltip, TooltipTrigger, TooltipContent } from '../components/ui/tooltip';
import DatasourceSelector from '../components/DatasourceSelector';
import useDatasources from '../hooks/useDatasources';
import NoDatasourcesEmptyState from '../components/NoDatasourcesEmptyState';

const LOCALSTORAGE_KEY = 'kyomi:sqlEditor:lastDatasourceSlug';

const SQLEditorPage = () => {
  const { showTour } = useProductTour();
  const [mobileSidebarOpen, setMobileSidebarOpen] = useState(false);
  // Initialize from localStorage if available
  const [selectedDatasourceSlug, setSelectedDatasourceSlug] = useState(() => {
    try {
      return localStorage.getItem(LOCALSTORAGE_KEY) || null;
    } catch {
      return null;
    }
  });
  const [selectedDatasource, setSelectedDatasource] = useState(null);

  // Datasources check for empty state
  const { hasDatasources, loading: datasourcesLoading } = useDatasources();

  // Show SQL editor tour on first load
  useEffect(() => {
    showTour('sqlEditor');
  }, [showTour]);

  // Handle datasource selection - store slug and full object, persist to localStorage
  // NOTE: All hooks must be called before any early return (React hooks rules)
  const handleDatasourceChange = useCallback((datasourceSlug, datasource) => {
    setSelectedDatasourceSlug(datasourceSlug);
    setSelectedDatasource(datasource);
    // Persist selection to localStorage
    try {
      if (datasourceSlug) {
        localStorage.setItem(LOCALSTORAGE_KEY, datasourceSlug);
      } else {
        localStorage.removeItem(LOCALSTORAGE_KEY);
      }
    } catch {
      // Ignore localStorage errors (e.g., private browsing)
    }
  }, []);

  // Query handler - uses unified queryService for all datasource types
  // NOTE: sql parameter is passed from SQLEditor (current editor content)
  // This prevents recreating the callback on every keystroke
  const handleRunQuery = useCallback(async (sql) => {
    const startTime = performance.now();
    let status = 'success';
    let errorMessage = null;
    let results = null;

    try {
      // Validate datasource is selected
      if (!selectedDatasourceSlug || !selectedDatasource?.datasource_type) {
        throw new Error('Please select a datasource before running a query');
      }

      // Unified query execution - no if/else for datasource type!
      results = await queryService.executeQuery(sql, {
        slug: selectedDatasourceSlug,
        type: selectedDatasource.datasource_type,
      }, {
        pageSize: 50,
      });

      return results;
    } catch (error) {
      status = 'error';
      errorMessage = error.message || 'Query execution failed';
      throw error;
    } finally {
      const endTime = performance.now();
      const executionTimeMs = Math.round(endTime - startTime);

      // Save to query history (fire and forget - don't block on errors)
      try {
        await apiClient.post('/api/v1/sql/history', {
          query_text: sql,
          execution_time_ms: executionTimeMs,
          bytes_processed: results?.bytesProcessed || null,
          // Use totalRows for server-side pagination, fallback to rows.length for client-side
          row_count: results?.totalRows || results?.rows?.length || null,
          status: status,
          error_message: errorMessage,
          datasource: selectedDatasourceSlug // Track which datasource was used (slug)
        });
      } catch (historyError) {
        // Don't fail the query execution if history save fails
      }
    }
  }, [selectedDatasourceSlug, selectedDatasource]); // Recreate when datasource changes

  // Show empty state if no datasources are configured
  // NOTE: This early return must be AFTER all hooks to comply with React hooks rules
  if (!datasourcesLoading && !hasDatasources) {
    return <NoDatasourcesEmptyState context="sql" />;
  }

  return (
      <div className="flex flex-col h-full bg-muted" style={{flexDirection: 'column'}}>
        {/* Header */}
        <div className="h-14 sm:h-16 border-b border-border bg-card px-4 sm:px-6 flex-shrink-0 flex items-center justify-between">
          <div className="flex items-center gap-2 sm:gap-4 min-w-0">
            <h1 className="text-lg sm:text-2xl font-semibold text-foreground shrink-0">SQL Editor</h1>

            {/* Datasource Selector */}
            <DatasourceSelector
              value={selectedDatasourceSlug}
              onChange={handleDatasourceChange}
            />
          </div>

          {/* Sidebar toggle button - all screen sizes */}
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => setMobileSidebarOpen(!mobileSidebarOpen)}
                className={`flex items-center gap-2 px-2 md:px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
                  mobileSidebarOpen
                    ? 'bg-primary/10 text-primary'
                    : 'bg-accent text-foreground hover:bg-accent'
                }`}
                aria-label="Toggle sidebar"
              >
                <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4m0 5c0 2.21-3.582 4-8 4s-8-1.79-8-4" />
                </svg>
                <span className="hidden sm:inline">Catalog</span>
              </button>
            </TooltipTrigger>
            <TooltipContent>Catalog & History</TooltipContent>
          </Tooltip>
        </div>

        {/* Content Area */}
        <div className="flex flex-1 min-h-0 relative">
          <SQLEditor
            onRunQuery={handleRunQuery}
            showCopyButton={true}
            mobileSidebarOpen={mobileSidebarOpen}
            onMobileSidebarClose={() => setMobileSidebarOpen(false)}
            datasourceSlug={selectedDatasourceSlug}
            selectedDatasourceType={selectedDatasource?.datasource_type}
            onDatasourceChange={setSelectedDatasourceSlug}
          />
        </div>
      </div>
  );
};

export default SQLEditorPage;
