// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ResultTab Component
 *
 * A single tab representing a query result with colored badge and minimal info.
 * Uses the centralized chart color palette for consistency.
 */

import { memo } from 'react';
import { Tooltip, TooltipTrigger, TooltipContent } from '../../../components/ui/tooltip';
import { CHART_PALETTES } from '../../../config/chartPalettes';
import useRenderLogger from '../../../hooks/useRenderLogger';

/**
 * Get tab color based on stored color index
 * Color is assigned when tab is created and stays with the tab
 * Uses the balanced palette for tab decoration
 */
const getTabColor = (colorIndex) => {
  const palette = CHART_PALETTES.balanced;
  return palette[colorIndex % palette.length];
};

/**
 * Format execution time
 */
const formatTime = (ms) => {
  if (!ms) return null;
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
};

/**
 * Format row count
 */
const formatRows = (count) => {
  if (!count && count !== 0) return null;
  if (count < 1000) return `${count}`;
  if (count < 1000000) return `${(count / 1000).toFixed(1)}K`;
  return `${(count / 1000000).toFixed(1)}M`;
};

/**
 * ResultTab component
 *
 * Displays a colored tab with minimal info (rows, time).
 *
 * Usage:
 * ```jsx
 * <ResultTab
 *   tab={tab}
 *   allTabs={tabs}
 *   isActive={true}
 *   onClick={() => setActiveTab(tab.id)}
 *   onClose={() => removeTab(tab.id)}
 *   onTogglePin={() => togglePin(tab.id)}
 * />
 * ```
 */
const ResultTab = ({ tab, allTabs, isActive, onClick, onClose, onTogglePin, editorRef, onDatasourceChange }) => {
  // Development: Log re-renders
  useRenderLogger('ResultTab', { tabId: tab.id, isActive, status: tab.status });

  const color = getTabColor(tab.colorIndex);

  const handleClose = (e) => {
    e.stopPropagation(); // Prevent tab selection when closing
    onClose();
  };

  const handleTogglePin = (e) => {
    e.stopPropagation(); // Prevent tab selection when pinning
    onTogglePin();
  };

  const handleDoubleClick = (e) => {
    e.stopPropagation(); // Prevent triggering single click
    // Restore SQL query to editor AND change datasource to match the tab
    if (editorRef?.current && tab.query) {
      editorRef.current.setValue(tab.query);
      // Also restore the datasource so the query runs against the correct datasource
      if (onDatasourceChange && tab.datasourceSlug) {
        onDatasourceChange(tab.datasourceSlug);
      }
    }
  };

  // Build info display based on status
  let info = null;
  if (tab.status === 'running') {
    info = 'Running...';
  } else if (tab.result && (tab.result.totalRows !== undefined || tab.result.rowCount !== undefined || tab.result.executionTime !== undefined)) {
    // Show result metadata if available, even if there's an error (e.g., expired results)
    // Use totalRows for server-side pagination, fallback to rowCount for client-side
    const rowCount = formatRows(tab.result.totalRows || tab.result.rowCount);
    const execTime = formatTime(tab.result.executionTime);
    info = [rowCount && `${rowCount} rows`, execTime].filter(Boolean).join(' · ');
  } else if (tab.status === 'error' || tab.error) {
    info = 'Error';
  }

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div
          onClick={onClick}
          onDoubleClick={handleDoubleClick}
          className={`
            flex items-center gap-2 px-3 py-2 border-r border-border cursor-pointer
            transition-all relative group min-w-0
            ${isActive
              ? 'bg-card'
              : 'bg-muted hover:bg-accent'
            }
          `}
        >
      {/* Colored circle indicator */}
      <div
        className="w-3 h-3 rounded-full flex-shrink-0 transition-opacity"
        style={{
          backgroundColor: color,
          opacity: isActive ? 1 : 0.6
        }}
      />

      {/* Info display */}
      {info && (
        <span className={`
          text-xs font-medium select-none whitespace-nowrap
          ${isActive ? 'text-foreground' : 'text-muted-foreground'}
        `}>
          {info}
        </span>
      )}

      {/* Status icon for running/error */}
      {tab.status === 'running' && (
        <svg className="w-3 h-3 animate-spin flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24" style={{ color }}>
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
        </svg>
      )}
      {(tab.status === 'error' || tab.error) && tab.status !== 'running' && (
        <svg className="w-3 h-3 text-info-foreground flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
        </svg>
      )}

      {/* Pin button - always show when pinned, show on hover when unpinned */}
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            onClick={handleTogglePin}
            className={`
              p-0.5 rounded hover:bg-muted transition-colors flex-shrink-0
              ${tab.pinned ? 'opacity-100' : (isActive ? 'opacity-100' : 'opacity-0 group-hover:opacity-100')}
            `}
            aria-label={tab.pinned ? "Unpin tab (will auto-close when limit reached)" : "Pin tab (keep permanently)"}
          >
            {tab.pinned ? (
              <svg className="w-3.5 h-3.5" fill="currentColor" viewBox="0 0 24 24" style={{ color }}>
                <path d="M16 9V4h1c.55 0 1-.45 1-1s-.45-1-1-1H7c-.55 0-1 .45-1 1s.45 1 1 1h1v5c0 1.66-1.34 3-3 3v2h5.97v7l1 1 1-1v-7H19v-2c-1.66 0-3-1.34-3-3z"/>
              </svg>
            ) : (
              <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 5a2 2 0 012-2h10a2 2 0 012 2v16l-7-3.5L5 21V5z" />
              </svg>
            )}
          </button>
        </TooltipTrigger>
        <TooltipContent>{tab.pinned ? "Unpin tab (will auto-close when limit reached)" : "Pin tab (keep permanently)"}</TooltipContent>
      </Tooltip>

      {/* Close button - only show on hover or active */}
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            onClick={handleClose}
            className={`
              p-0.5 rounded hover:bg-accent transition-colors flex-shrink-0
              ${isActive ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'}
            `}
            aria-label="Close tab"
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </TooltipTrigger>
        <TooltipContent>Close tab</TooltipContent>
      </Tooltip>

      {/* Active tab indicator (bottom border) */}
      {isActive && (
        <div
          className="absolute bottom-0 left-0 right-0 h-0.5"
          style={{ backgroundColor: color }}
        />
      )}
        </div>
      </TooltipTrigger>
      <TooltipContent>
        <div className="text-xs max-w-xs">
          <div className="font-mono mb-2">{tab.query}</div>
          <div className="text-muted-foreground">Click to view • Double-click to restore SQL</div>
        </div>
      </TooltipContent>
    </Tooltip>
  );
};

// Custom comparison function to prevent unnecessary re-renders
// Only re-render if the tab's own properties or isActive state has changed
const arePropsEqual = (prevProps, nextProps) => {
  // If isActive changed for this specific tab, re-render
  if (prevProps.isActive !== nextProps.isActive) {
    return false;
  }

  // If tab object reference changed, re-render
  if (prevProps.tab !== nextProps.tab) {
    return false;
  }

  // Color is now stored on the tab itself (tab.colorIndex), not based on array position
  // So no need to check allTabs changes for color updates

  // Callbacks are stable (memoized), so no need to compare them
  // If none of the above changed, skip re-render
  return true;
};

export default memo(ResultTab, arePropsEqual);
