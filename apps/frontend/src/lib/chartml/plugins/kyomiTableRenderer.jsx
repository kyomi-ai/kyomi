// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Kyomi Table Chart Renderer Plugin for ChartML
 *
 * Wraps ResizableTable React component as a ChartML chart renderer.
 * Provides sorting, pagination, column resizing, and CSV download.
 *
 * This is a PROPRIETARY Kyomi plugin that uses React for advanced table features.
 */

import { createRoot } from 'react-dom/client';
import { useState } from 'react';
import ResizableTable from '../../../components/ResizableTable.jsx';

/**
 * Wrapper component to manage pagination and sorting state for table charts
 * Uses ChartML's onSpecChange callback to request sorted data from middleware
 */
function TableWrapper({ data, height, initialPageSize = 50, spec, onSpecChange }) {
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(initialPageSize);
  const [sortBy, setSortBy] = useState([]);

  // Handle sort change from ResizableTable
  const handleSortChange = (column, isCtrlClick) => {
    setSortBy(prevSort => {
      let newSort;
      const existingIndex = prevSort.findIndex(s => s.column === column);

      if (!isCtrlClick) {
        // Regular click: single column sort
        if (existingIndex >= 0) {
          const currentSort = prevSort[existingIndex];
          if (currentSort.direction === 'asc') {
            newSort = [{ column, direction: 'desc' }];
          } else {
            newSort = []; // Clear sort
          }
        } else {
          newSort = [{ column, direction: 'asc' }];
        }
      } else {
        // Ctrl+click: multi-column sort
        if (existingIndex >= 0) {
          const currentSort = prevSort[existingIndex];
          if (currentSort.direction === 'asc') {
            newSort = prevSort.map((s, i) =>
              i === existingIndex ? { ...s, direction: 'desc' } : s
            );
          } else {
            newSort = prevSort.filter((_, i) => i !== existingIndex);
          }
        } else {
          newSort = [...prevSort, { column, direction: 'asc' }];
        }
      }

      // Reset to page 1 when sort changes
      setCurrentPage(1);

      // Request re-render with modified spec via ChartML callback (async, non-blocking)
      // Pass ONLY the modifications (transform.aggregate.sort), ChartML core will merge with originalSpec
      if (onSpecChange && newSort.length > 0) {
        const modifications = {
          transform: {
            aggregate: {
              sort: newSort.map(s => ({
                field: s.column,
                direction: s.direction
              }))
            }
          }
        };
        onSpecChange(modifications).catch(() => {});
      } else if (onSpecChange && newSort.length === 0) {
        // Clear sort
        const modifications = {
          transform: {
            aggregate: {
              sort: undefined
            }
          }
        };
        onSpecChange(modifications).catch(() => {});
      }

      return newSort;
    });
  };

  // Apply height constraint if specified in ChartML style
  // Use flex column layout to properly constrain ResizableTable's h-full
  const containerStyle = height ? { height, maxHeight: height } : {};

  return (
    <div style={containerStyle} className={height ? 'flex flex-col overflow-hidden' : ''}>
      <ResizableTable
        data={data}
        showHeader={false}
        enableResize={true}
        sortBy={sortBy}
        onSortChange={handleSortChange}
        currentPage={currentPage}
        pageSize={pageSize}
        onPageChange={setCurrentPage}
        onPageSizeChange={(newSize) => {
          setPageSize(newSize);
          setCurrentPage(1); // Reset to first page when page size changes
        }}
        variant="markdown"
      />
    </div>
  );
}

/**
 * Create Kyomi table renderer
 *
 * @returns {Function} Renderer function compatible with ChartML
 *
 * @example
 * const chartml = new ChartML();
 * const tableRenderer = createKyomiTableRenderer();
 * chartml.registerChartRenderer('table', tableRenderer);
 */
export function createKyomiTableRenderer() {
  /**
   * Render an interactive table with sorting and pagination
   *
   * @param {HTMLElement} container - DOM element to render into
   * @param {Array<Object>} data - Chart data (array of objects)
   * @param {Object} config - Chart configuration from ChartML mapper
   * @param {Object} config.spec - Original visualizeSpec from ChartML
   */
  return function renderTable(container, data, config) {
    // Extract columns from ChartML visualize spec
    const { spec } = config;
    const visualizeColumns = spec.columns;

    // Determine which columns to display
    let columns, columnLabels;

    if (visualizeColumns && Array.isArray(visualizeColumns)) {
      // visualize.columns can be:
      // 1. Array of strings: ['date', 'count', 'value']
      // 2. Array of objects: [{field: 'date', label: 'Date'}, {field: 'count', label: 'Count'}]

      const firstCol = visualizeColumns[0];

      if (typeof firstCol === 'object' && firstCol.field) {
        // Case 2: Array of column objects with field/label
        columns = visualizeColumns.map(col => col.field);
        columnLabels = visualizeColumns.map(col => col.label || col.field);
      } else {
        // Case 1: Array of strings (simple column names)
        columns = visualizeColumns;
        columnLabels = visualizeColumns;
      }
    } else {
      // No column specification - show all columns from data
      const allColumns = data.length > 0 ? Object.keys(data[0]) : [];
      columns = allColumns;
      columnLabels = allColumns;
    }

    // Convert data to ResizableTable format
    // ChartML data: [{col: val}, {col: val}, ...]
    // ResizableTable expects: { columns: [...], rows: [[...]], row_count: N }
    const rows = data.map(row => columns.map(col => row[col]));

    const tableData = {
      columns: columnLabels,
      rows: rows,
      row_count: data.length
    };

    // Extract style attributes from ChartML spec
    // Height can be in visualize.style.height (spec is the full resolved spec)
    // Default table height is 400px (same as standard charts)
    // Note: ChartML core renders the title separately, so we don't add title height here
    const DEFAULT_TABLE_HEIGHT = 400;
    const visualizeStyle = spec.visualize?.style || spec.style || {};
    const height = visualizeStyle.height || DEFAULT_TABLE_HEIGHT;
    const initialPageSize = visualizeStyle.pageSize || 50;

    // Create or reuse a wrapper div for React content
    // This allows ChartML's title to exist as a sibling (title is inserted at container.firstChild)
    // Without this wrapper, React's root.render() would overwrite the title
    let reactWrapper = container.querySelector('.table-react-wrapper');
    if (!reactWrapper) {
      reactWrapper = document.createElement('div');
      reactWrapper.className = 'table-react-wrapper';
      container.appendChild(reactWrapper);
    }

    // Reuse existing React root if possible (prevents ResizeObserver loops in KyomiChart)
    let root = reactWrapper._reactRoot;
    if (!root) {
      // First render - create new root
      root = createRoot(reactWrapper);
      reactWrapper._reactRoot = root;
    }

    // Update table data via root.render() (doesn't trigger container resize)
    // Pass spec and onSpecChange callback for sorting support
    root.render(
      <TableWrapper
        data={tableData}
        height={height}
        initialPageSize={initialPageSize}
        spec={spec}
        onSpecChange={config.onSpecChange}
      />
    );
  };

  // Implement ChartML plugin interface for default dimensions
  // Tables have a default height of 400px with scrollable content
  // Read from spec if provided, otherwise use default
  // Note: includesTitle: false because ChartML core renders title separately
  renderTable.getDefaultDimensions = (spec) => {
    const DEFAULT_TABLE_HEIGHT = 400;
    const visualizeStyle = spec?.visualize?.style || spec?.style || {};
    const height = visualizeStyle.height || DEFAULT_TABLE_HEIGHT;
    return { height, includesTitle: false };
  };

  return renderTable;
}

export default createKyomiTableRenderer;
