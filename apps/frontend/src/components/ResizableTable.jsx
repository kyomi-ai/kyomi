// SPDX-License-Identifier: AGPL-3.0-or-later
import { useEffect, useRef, useState, useMemo, useCallback, memo } from 'react';
import useRenderLogger from '../hooks/useRenderLogger';
import './ResizableTable.css';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select';
import { formatCellValue } from './utils/cellFormatter.jsx';
import { Spinner } from './ui/spinner';

/**
 * ResizableTable - A native HTML table with fixed header, scrollable rows, and resizable columns
 *
 * Features:
 * - Fixed sticky header that stays visible during scroll
 * - Scrollable rows with alternating colors
 * - Resizable columns via drag handles
 * - Perfect text selection that works reliably across all browsers
 * - Smart auto-sizing: fills container width by default, switches to manual control after user resize
 * - Lazy loading: When cacheKey is provided, fetches pages on-demand from DuckDB
 * - Column sorting: Click to sort, ctrl+click to add secondary/tertiary sorts
 * - CSV download: Download entire dataset to CSV file
 *
 * @param {Object} props
 * @param {Object} props.data - Table data with { columns: string[], rows: any[][], row_count: number }
 * @param {number} props.data.row_count - Total number of rows (for display)
 * @param {string[]} props.data.columns - Array of column names
 * @param {any[][]} props.data.rows - Array of row data (each row is an array of cell values)
 * @param {string} props.cacheKey - Optional cache key for lazy loading from DuckDB
 * @param {Array} props.series - Optional array of series objects with {y: columnName, name: friendlyName, width: {mode, value}}
 * @param {Function} props.onClose - Optional callback when close button is clicked
 * @param {boolean} props.showHeader - Optional, show header with row/column count and close button (default: true)
 * @param {boolean} props.enableResize - Optional, enable column resizing via drag handles (default: true)
 * @param {Function} props.onPageChange - Optional callback when page changes
 * @param {number} props.currentPage - Current page number (default: 1)
 * @param {Array} props.sortBy - Array of {column, direction} objects for sorting
 * @param {Function} props.onSortChange - Callback when sort changes (column, isCtrlClick)
 * @param {Function} props.onDownloadCSV - Optional callback to download full dataset as CSV
 * @param {number} props.pageSize - Number of rows per page (default: 50)
 * @param {Function} props.onPageSizeChange - Optional callback when page size changes
 * @param {boolean} props.isPaginating - Whether pagination is in progress (shows overlay instead of replacing table)
 * @param {React.ReactNode} props.headerActions - Optional additional actions to render in the header (e.g., Create Chart button)
 * @param {string} props.variant - Visual style variant: 'default' (SQL editor style) or 'markdown' (chat/dashboard style) (default: 'default')
 */
const ResizableTable = ({ data, series = [], onClose, showHeader = true, enableResize = true, onPageChange, currentPage = 1, sortBy = [], onSortChange, onDownloadCSV, pageSize = 50, onPageSizeChange, isPaginating = false, headerActions, variant = 'default' }) => {
  // Development: Log re-renders
  useRenderLogger('ResizableTable', {
    rowCount: data?.rows?.length || 0,
    columnCount: data?.columns?.length || 0,
    currentPage,
    pageSize
  });

  const [columnWidths, setColumnWidths] = useState({});
  const [resizing, setResizing] = useState(null);
  const [userResized, setUserResized] = useState(false);
  const tableRef = useRef(null);
  const containerRef = useRef(null);
  const capturedWidthsRef = useRef(null); // Store captured widths without triggering re-render

  // Calculate pagination - use row_count if provided (server-side pagination), otherwise length
  const totalRows = data.row_count || data.rows.length;
  const totalPages = Math.ceil(totalRows / pageSize);

  // Detect server-side vs client-side pagination
  // Server-side: data.row_count exists and is larger than data.rows.length (server sends only current page)
  // Client-side: data.row_count === data.rows.length (all data is in memory)
  const isServerSidePagination = data.row_count && data.row_count > data.rows.length;

  // For client-side pagination, calculate slice indices
  // For server-side pagination, we render all rows (server already sent the right page)
  const startIdx = isServerSidePagination ? 0 : (currentPage - 1) * pageSize;
  const endIdx = isServerSidePagination ? data.rows.length : Math.min(startIdx + pageSize, totalRows);

  // Calculate display indices (what to show to user in "Showing X-Y of Z")
  const displayStartIdx = isServerSidePagination ? ((currentPage - 1) * pageSize) : startIdx;
  const displayEndIdx = isServerSidePagination ? ((currentPage - 1) * pageSize + data.rows.length) : endIdx;

  // Reorder columns and rows based on series order - memoized to prevent infinite loops
  const orderedData = useMemo(() => {
    if (!series || series.length === 0) {
      // No series - data.rows contains page data from server-side pagination
      return {
        ...data,
        rows: data.rows,
        totalRows: data.row_count || data.rows.length
      };
    }

    // Helper to extract column name from string or object
    const getColName = (col) => typeof col === 'string' ? col : (col.name || col.value || String(col));

    // Create ordered columns array based on series order - ONLY include columns from series
    const orderedColumns = series.map(s => s.y);

    // Create column index mapping for row reordering
    const columnIndexMap = orderedColumns.map(col => {
      const colName = getColName(col);
      return data.columns.findIndex(c => getColName(c) === colName);
    });

    // Reorder rows - data.rows already contains page data from server-side pagination
    const orderedRows = data.rows.map(row =>
      columnIndexMap.map(idx => row[idx])
    );

    return {
      ...data,
      columns: orderedColumns,
      rows: orderedRows,
      totalRows: data.row_count || data.rows.length
    };
  }, [data, series]);

  // Calculate flex-based column styles based on width configuration
  const getColumnStyle = useCallback((col) => {
    const seriesConfig = series.find(s => s.y === col);
    const widthConfig = seriesConfig?.width || { mode: 'auto' };

    // If user has manually resized via drag, always use that (temporary override)
    if (userResized && columnWidths[col]) {
      return {
        flex: `0 0 ${columnWidths[col]}px`,
        width: `${columnWidths[col]}px`,
        minWidth: `${columnWidths[col]}px`,
        maxWidth: `${columnWidths[col]}px`
      };
    }

    // Otherwise use configured width mode
    switch (widthConfig.mode) {
      case 'fixed':
        return {
          flex: `0 0 ${widthConfig.value}px`,
          width: `${widthConfig.value}px`,
          minWidth: `${widthConfig.value}px`,
          maxWidth: `${widthConfig.value}px`
        };
      case 'percent':
        return {
          flex: `0 0 ${widthConfig.value}%`,
          width: `${widthConfig.value}%`,
          minWidth: '50px' // Prevent too narrow
        };
      case 'auto':
      default:
        return {
          flex: '1 1 0',
          minWidth: '100px' // Prevent too narrow
        };
    }
  }, [series, userResized, columnWidths]);

  // Pre-compute ONLY dynamic width styles (static styles moved to CSS classes)
  const headerDynamicStyles = useMemo(() => {
    const styles = {};
    orderedData.columns.forEach((col) => {
      const colName = typeof col === 'string' ? col : (col.name || col.value || String(col));
      styles[colName] = getColumnStyle(colName); // Only flex/width calculations
    });
    return styles;
  }, [orderedData.columns, getColumnStyle]);

  const cellDynamicStyles = useMemo(() => {
    const styles = {};
    orderedData.columns.forEach((col) => {
      const colName = typeof col === 'string' ? col : (col.name || col.value || String(col));
      styles[colName] = getColumnStyle(colName); // Only flex/width calculations
    });
    return styles;
  }, [orderedData.columns, getColumnStyle]);

  useEffect(() => {
    // Initialize column widths only if columns have changed
    const currentColumns = Object.keys(columnWidths);
    const newColumns = orderedData.columns;

    // Check if columns are different
    const columnsChanged =
      currentColumns.length !== newColumns.length ||
      newColumns.some(col => !currentColumns.includes(col));

    if (columnsChanged) {
      const initialWidths = {};
      orderedData.columns.forEach(col => {
        initialWidths[col] = 150; // Default width
      });
      setColumnWidths(initialWidths);
      setUserResized(false); // Reset when data changes
    }
  }, [orderedData.columns]);

  // Handle column click for sorting
  const handleColumnClick = useCallback((column, e) => {
    // Don't trigger sort if clicking the resize handle
    if (e.target.closest('.resize-handle')) {
      return;
    }

    if (onSortChange) {
      onSortChange(column, e.ctrlKey || e.metaKey);
    }
  }, [onSortChange]);

  // Get sort indicator for a column
  const getSortIndicator = useCallback((column) => {
    const sortIndex = sortBy.findIndex(s => s.column === column);
    if (sortIndex === -1) return null;

    const sort = sortBy[sortIndex];
    const arrow = sort.direction === 'asc' ? '↑' : '↓';
    const number = sortBy.length > 1 ? sortIndex + 1 : '';

    return (
      <span className="ml-1 text-primary font-bold">
        {arrow}{number}
      </span>
    );
  }, [sortBy]);

  const handleMouseDown = useCallback((e, column) => {
    e.preventDefault();
    e.stopPropagation(); // Prevent triggering column click

    // Get current actual width of the column being resized
    const th = e.target.closest('th');
    const currentWidth = th ? th.getBoundingClientRect().width : 150;

    // If this is the first resize, capture ALL column widths without triggering re-render
    if (!userResized && tableRef.current) {
      const ths = tableRef.current.querySelectorAll('thead th');
      const actualWidths = {};
      ths.forEach((th, idx) => {
        if (idx === 0) return; // Skip row number column
        const col = orderedData.columns[idx - 1]; // -1 because of row number column
        const colName = typeof col === 'string' ? col : (col.name || col.value || String(col));
        actualWidths[colName] = th.getBoundingClientRect().width;
      });
      capturedWidthsRef.current = actualWidths;
      setResizing({ column, startX: e.clientX, startWidth: currentWidth, isFirstResize: true });
    } else {
      setResizing({ column, startX: e.clientX, startWidth: currentWidth });
    }
  }, [userResized, orderedData.columns]);

  useEffect(() => {
    if (!resizing) return;

    const handleMouseMove = (e) => {
      // On first mousemove during first-ever resize, apply captured widths
      if (resizing.isFirstResize && capturedWidthsRef.current) {
        setColumnWidths(capturedWidthsRef.current);
        setUserResized(true);
        capturedWidthsRef.current = null; // Clear ref
        setResizing(prev => ({ ...prev, isFirstResize: false })); // Clear flag
      }

      const diff = e.clientX - resizing.startX;
      const newWidth = Math.max(50, resizing.startWidth + diff);
      setColumnWidths(prev => ({ ...prev, [resizing.column]: newWidth }));
    };

    const handleMouseUp = () => {
      setResizing(null);
    };

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);

    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
    };
  }, [resizing]);

  // Determine if markdown variant
  const isMarkdown = variant === 'markdown';

  return (
    <div className={`flex-1 min-h-0 h-full flex flex-col rounded-md overflow-hidden relative ${
      isMarkdown
        ? 'bg-transparent'
        : 'border border-border bg-card'
    }`}>
      {showHeader && (
        <div className={`px-4 py-3 border-b text-sm flex-shrink-0 flex items-center justify-between ${
          isMarkdown
            ? 'bg-transparent border-border text-muted-foreground'
            : 'bg-muted border-border text-muted-foreground'
        }`}>
          <span>
            Results: Showing {(displayStartIdx + 1).toLocaleString()}-{displayEndIdx.toLocaleString()} of <strong>{totalRows.toLocaleString()}</strong> row{totalRows !== 1 ? 's' : ''} × {orderedData.columns?.length || 0} column{orderedData.columns?.length !== 1 ? 's' : ''}
          </span>
          <div className="flex items-center gap-2">
            {headerActions}
            {onDownloadCSV && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={onDownloadCSV}
                    className="px-3 py-1 bg-primary text-white rounded hover:bg-primary/90 transition-colors text-xs font-medium flex items-center gap-1"
                    aria-label="Download entire dataset as CSV"
                  >
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
                    </svg>
                    Download CSV
                  </button>
                </TooltipTrigger>
                <TooltipContent>Download entire dataset as CSV</TooltipContent>
              </Tooltip>
            )}
            {onClose && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={onClose}
                    className="text-muted-foreground hover:text-foreground transition-colors"
                    aria-label="Close preview"
                  >
                    <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                    </svg>
                  </button>
                </TooltipTrigger>
                <TooltipContent>Close preview</TooltipContent>
              </Tooltip>
            )}
          </div>
        </div>
      )}
      {/* Pagination loading overlay - positioned absolutely over scrollable area */}
      {isPaginating && (
        <div className="absolute inset-0 bg-card/70 z-20 flex items-center justify-center pointer-events-none">
          <div className="flex items-center gap-3 bg-card px-4 py-3 rounded-lg shadow-lg border border-border pointer-events-auto">
            <Spinner size="md" className="text-primary" />
            <span className="text-xs text-muted-foreground">Loading...</span>
          </div>
        </div>
      )}
      <div ref={containerRef} className={`flex-1 overflow-auto relative ${isMarkdown ? 'text-sm' : 'text-xs'}`}>
        <table ref={tableRef} className="border-collapse" style={{ tableLayout: 'fixed', width: '100%', display: 'table' }}>
          <thead className="sticky top-0 z-10">
            <tr className="resizable-table-row">
              {/* Row number header - narrow column with no title (only for default variant) */}
              {!isMarkdown && (
                <th
                  className="resizable-table-row-number-header bg-accent border-b border-r-2 border-input text-center font-normal text-muted-foreground"
                >
                  {/* Empty - no title for line numbers */}
                </th>
              )}
              {orderedData.columns.map((col, idx) => {
                // Handle both string column names and column objects {name, type} or {type, value}
                const colName = typeof col === 'string' ? col : (col.name || col.value || String(col));
                const isSortable = !!onSortChange;
                return (
                  <Tooltip key={idx}>
                    <TooltipTrigger asChild>
                      <th
                        className={`resizable-table-header text-left relative transition-colors ${
                          isSortable ? 'cursor-pointer' : ''
                        } ${
                          isMarkdown
                            ? `markdown-variant bg-background border-b-2 border-border font-semibold text-muted-foreground ${isSortable ? 'hover:bg-accent/50' : ''}`
                            : `bg-accent border-b border-border font-semibold text-foreground ${isSortable ? 'hover:bg-accent/80' : ''}`
                        }`}
                        style={headerDynamicStyles[colName]}
                        onClick={isSortable ? (e) => handleColumnClick(colName, e) : undefined}
                        aria-label={isSortable ? `Click to sort by ${colName}. Ctrl+click to add to sort.` : colName}
                      >
                        <div className="truncate pr-2 flex items-center">
                          {series.find(s => s.y === colName)?.name || colName}
                          {getSortIndicator(colName)}
                        </div>
                        {enableResize && (
                          <div
                            onMouseDown={(e) => handleMouseDown(e, colName)}
                            className="resize-handle resizable-table-resize-handle absolute right-0 top-0 bottom-0 cursor-col-resize"
                          >
                            <div className="resizable-table-resize-handle-inner bg-border hover:bg-muted-foreground" />
                          </div>
                        )}
                      </th>
                    </TooltipTrigger>
                    <TooltipContent>{isSortable ? `Click to sort by ${colName}. Ctrl+click to add to sort.` : colName}</TooltipContent>
                  </Tooltip>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {orderedData.rows.slice(startIdx, endIdx).map((row, rowIdx) => {
              // Row number bg inherits from parent row (handled by Tailwind classes)
              // Calculate actual row number: for server-side pagination, use currentPage offset
              const actualRowNumber = isServerSidePagination
                ? ((currentPage - 1) * pageSize) + rowIdx + 1
                : startIdx + rowIdx + 1;
              return (
                <tr key={rowIdx} className={`resizable-table-row ${
                  isMarkdown
                    ? 'bg-transparent hover:bg-accent/20'
                    : (rowIdx % 2 === 1 ? 'bg-muted' : 'bg-card')
                }`}>
                  {/* Row number cell - styled like code editor line numbers (only for default variant) */}
                  {!isMarkdown && (
                    <td
                      className="resizable-table-row-number-cell border-b border-r-2 border-border text-center text-muted-foreground select-none"
                    >
                      {actualRowNumber}
                    </td>
                  )}
                  {row.map((cell, cellIdx) => {
                    const col = orderedData.columns[cellIdx];
                    const colName = typeof col === 'string' ? col : (col.name || col.value || String(col));
                    const colType = typeof col === 'object' ? col.type : undefined;

                    // Format cell value using the formatter utility
                    const { displayValue, fullValue, isComplex } = formatCellValue(cell, colType);

                    return (
                      <Tooltip key={cellIdx}>
                        <TooltipTrigger asChild>
                          <td
                            className={`resizable-table-cell ${
                              isMarkdown
                                ? 'markdown-variant border-b border-border text-foreground'
                                : 'border-b border-accent text-foreground'
                            }`}
                            style={cellDynamicStyles[colName]}
                          >
                            {displayValue}
                          </td>
                        </TooltipTrigger>
                        <TooltipContent>
                          {isComplex ? (
                            <pre className="max-w-md max-h-96 overflow-auto text-xs whitespace-pre-wrap break-words">
                              {fullValue}
                            </pre>
                          ) : (
                            fullValue
                          )}
                        </TooltipContent>
                      </Tooltip>
                    );
                  })}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {/* Pagination toolbar */}
      <div className={`px-2 sm:px-4 py-2 border-t flex items-center justify-between flex-shrink-0 min-w-0 ${
        isMarkdown
          ? 'bg-transparent border-border'
          : 'bg-muted border-border'
      }`}>
        <div className={`flex items-center gap-1 sm:gap-2 text-xs whitespace-nowrap ${
          isMarkdown ? 'text-muted-foreground' : 'text-muted-foreground'
        }`}>
          <span className="hidden sm:inline">Rows per page:</span>
          <Select
            value={String(pageSize)}
            onValueChange={(value) => onPageSizeChange && onPageSizeChange(Number(value))}
          >
            <SelectTrigger className="w-[60px] sm:w-[80px] h-7 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="10">10</SelectItem>
              <SelectItem value="25">25</SelectItem>
              <SelectItem value="50">50</SelectItem>
              <SelectItem value="100">100</SelectItem>
              <SelectItem value="200">200</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="flex items-center gap-2 sm:gap-4 min-w-0">
          <div className={`text-xs whitespace-nowrap overflow-hidden text-ellipsis ${
            isMarkdown ? 'text-muted-foreground' : 'text-muted-foreground'
          }`}>
            <span className="hidden sm:inline">{(displayStartIdx + 1).toLocaleString()}-{displayEndIdx.toLocaleString()} of {totalRows.toLocaleString()}</span>
            <span className="sm:hidden">{(displayStartIdx + 1).toLocaleString()}-{displayEndIdx.toLocaleString()}</span>
          </div>
          <div className="flex items-center gap-1 flex-shrink-0">
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => onPageChange && onPageChange(1)}
                  disabled={currentPage === 1}
                  className="p-1 rounded text-muted-foreground hover:bg-foreground/10 disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                  aria-label="First page"
                >
                  <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 19l-7-7 7-7m8 14l-7-7 7-7" />
                  </svg>
                </button>
              </TooltipTrigger>
              <TooltipContent>First page</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => onPageChange && onPageChange(Math.max(1, currentPage - 1))}
                  disabled={currentPage === 1}
                  className="p-1 rounded text-muted-foreground hover:bg-foreground/10 disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                  aria-label="Previous page"
                >
                  <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
                  </svg>
                </button>
              </TooltipTrigger>
              <TooltipContent>Previous page</TooltipContent>
            </Tooltip>
            <span className={`text-xs px-2 ${
              isMarkdown ? 'text-muted-foreground' : 'text-muted-foreground'
            }`}>
              Page {currentPage} of {totalPages}
            </span>
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => onPageChange && onPageChange(Math.min(totalPages, currentPage + 1))}
                  disabled={currentPage === totalPages}
                  className="p-1 rounded text-muted-foreground hover:bg-foreground/10 disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                  aria-label="Next page"
                >
                  <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
                  </svg>
                </button>
              </TooltipTrigger>
              <TooltipContent>Next page</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => onPageChange && onPageChange(totalPages)}
                  disabled={currentPage === totalPages}
                  className="p-1 rounded text-muted-foreground hover:bg-foreground/10 disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                  aria-label="Last page"
                >
                  <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 5l7 7-7 7M5 5l7 7-7 7" />
                  </svg>
                </button>
              </TooltipTrigger>
              <TooltipContent>Last page</TooltipContent>
            </Tooltip>
          </div>
        </div>
      </div>
    </div>
  );
};

export default memo(ResizableTable);
