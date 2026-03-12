// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ResultsTable Component
 *
 * Displays query results in a resizable, sortable, paginated table.
 * Integrates with the SQL editor Zustand store for state management.
 */

import { useMemo, memo, useCallback } from 'react';
import ResizableTable from '../../../components/ResizableTable';

/**
 * Client-side sorting disabled for v1.0
 * Sorting requires server-side implementation (re-executing query with ORDER BY)
 *
 * Note: The applySorting function has been removed as it only sorted the current page,
 * which gave users a false impression of sorting across the entire dataset.
 */

/**
 * ResultsTable component
 *
 * Usage:
 * ```jsx
 * <ResultsTable
 *   result={queryResult}
 *   sortBy={sortBy}
 *   onSortChange={handleSort}
 *   currentPage={page}
 *   onPageChange={setPage}
 * />
 * ```
 */
const ResultsTable = ({
  result,
  sortBy,
  onSortChange,
  currentPage,
  onPageChange,
  pageSize = 50,
  onPageSizeChange,
  isPaginating = false,
  headerActions,
}) => {
  // Sorting disabled - no client-side sorting applied
  // Pass onSortChange directly (will be undefined when sorting is disabled)

  // Transform our QueryResult format to ResizableTable's expected format
  const tableData = useMemo(() => ({
    columns: result.columns,
    rows: result.rows,
    row_count: result.totalRows || result.rowCount || result.rows.length,
  }), [result]);

  return (
    <ResizableTable
      data={tableData}
      sortBy={sortBy}
      onSortChange={onSortChange}
      currentPage={currentPage}
      onPageChange={onPageChange}
      pageSize={pageSize}
      onPageSizeChange={onPageSizeChange}
      showHeader={true}
      enableResize={true}
      isPaginating={isPaginating}
      headerActions={headerActions}
    />
  );
};

// Custom comparison to prevent unnecessary re-renders
const arePropsEqual = (prev, next) => {
  // Re-render if result data identity changes
  if (prev.result !== next.result) return false;

  // Re-render if pagination changes
  if (prev.currentPage !== next.currentPage) return false;
  if (prev.pageSize !== next.pageSize) return false;

  // Re-render if sort changes
  if (prev.sortBy.length !== next.sortBy.length) return false;
  if (!prev.sortBy.every((s, i) =>
    next.sortBy[i] &&
    s.column === next.sortBy[i].column &&
    s.direction === next.sortBy[i].direction
  )) return false;

  // Re-render if pagination state changes
  if (prev.isPaginating !== next.isPaginating) return false;

  // Callbacks are stable, ignore them
  // HeaderActions might change, check identity
  if (prev.headerActions !== next.headerActions) return false;

  return true;
};

export default memo(ResultsTable, arePropsEqual);
