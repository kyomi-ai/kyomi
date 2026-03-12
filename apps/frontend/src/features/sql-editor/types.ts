/**
 * Type definitions for SQL Editor state management
 *
 * This file defines the core types for the tabbed SQL editor architecture,
 * including query results, visualizations, and tab management.
 */

/**
 * Column metadata from query execution
 */
export interface ColumnMetadata {
  name: string;
  type?: string; // Simplified type (string, number, boolean, datetime)
  mode?: string; // NULLABLE, REQUIRED, REPEATED
}

/**
 * Opaque handle for pagination - used by queryService.fetchPage()
 * Contains all info needed to fetch additional pages from any datasource type.
 */
export interface QueryHandle {
  datasourceType: string;   // 'bigquery', 'postgres', 'clickhouse', etc.
  datasourceSlug: string;   // 'production-postgres'
  sql: string;              // Original query (for re-execution)
  jobId?: string;           // BigQuery-specific: job ID for random page access
}

/**
 * Represents the result of a query execution
 */
export interface QueryResult {
  columns: string[] | ColumnMetadata[]; // Support both legacy string[] and new metadata format
  rows: any[][];
  rowCount: number;
  totalRows?: number; // Total rows available (for server-side pagination)
  queryHandle?: QueryHandle; // Unified pagination handle (works for all datasource types)
  executionTime?: number;
  bytesProcessed?: number;
}

/**
 * Represents an error from query execution
 */
export interface QueryError {
  message: string;
  code?: string;
  line?: number;
  column?: number;
}

/**
 * Status of a query execution
 */
export type QueryStatus = 'idle' | 'running' | 'streaming' | 'success' | 'error';

/**
 * Represents a ChartML visualization configuration
 */
export interface Visualization {
  id: string;
  chartML: any; // ChartML object (YAML parsed)
  chartMLText?: string; // YAML string representation
  title?: string;
  createdAt: number;
}

/**
 * A single result tab containing query results and optional visualization
 */
export interface ResultTab {
  id: string;
  label: string;
  query: string;
  status: QueryStatus;
  result?: QueryResult;
  error?: QueryError;
  visualization?: Visualization;
  pinned: boolean; // Pinned tabs are kept beyond the 3-tab limit
  colorIndex: number; // Stable color index for this tab (doesn't change when other tabs are removed)
  createdAt: number;
  updatedAt: number;
  needsRefresh?: boolean; // Tab was loaded from localStorage and needs data refresh
  datasourceSlug?: string; // Datasource slug (e.g., "production-postgres")
  datasourceType?: string; // Datasource type (e.g., "bigquery", "postgres")
}

/**
 * Sort configuration for table columns
 */
export interface ColumnSort {
  column: string;
  direction: 'asc' | 'desc';
}

/**
 * UI state for the results table
 */
export interface TableUIState {
  sortBy: ColumnSort[];
  currentPage: number;
  pageSize: number;
}

/**
 * The complete SQL editor state
 */
export interface SqlEditorState {
  // Tab management
  tabs: ResultTab[];
  activeTabId: string | null;
  nextColorIndex: number; // Next color index to assign (0-7, cycles)

  // SQL query text (shared across UI, separate from tab results)
  queryText: string;

  // UI state per tab
  tableUIState: Record<string, TableUIState>;

  // Global user preferences (persisted)
  defaultPageSize: number; // User's preferred page size for new tabs

  // Sidebar state (persisted)
  activeRightTab: string | null; // 'catalog', 'history', 'details', or null (closed)
  rightSidebarPercentage: number; // Width percentage (0-50)

  // Actions
  addTab: (tab: Omit<ResultTab, 'id' | 'createdAt' | 'updatedAt' | 'pinned' | 'colorIndex'>) => string;
  updateTab: (tabId: string, updates: Partial<ResultTab>) => void;
  removeTab: (tabId: string) => void;
  setActiveTab: (tabId: string | null) => void;
  togglePin: (tabId: string) => void; // Toggle pinned state

  setQueryText: (text: string) => void;

  setTableUIState: (tabId: string, state: Partial<TableUIState>) => void;
  setDefaultPageSize: (pageSize: number) => void; // Update global page size preference

  setActiveRightTab: (tab: string | null) => void; // Set active right sidebar tab
  setRightSidebarPercentage: (percentage: number) => void; // Set right sidebar width

  clearAllTabs: () => void;
}

/**
 * Props for ResultsTable component
 */
export interface ResultsTableProps {
  result: QueryResult;
  sortBy: ColumnSort[];
  onSortChange: (column: string, isCtrlClick: boolean) => void;
  currentPage: number;
  onPageChange: (page: number) => void;
  pageSize?: number;
  isPaginating?: boolean;
}

/**
 * Props for ResultsError component
 */
export interface ResultsErrorProps {
  error: QueryError;
  onRerun?: () => void;
  rerunning?: boolean;
}

/**
 * Props for ResultsLoading component
 */
export interface ResultsLoadingProps {
  message?: string;
}

/**
 * Props for ResultTab component
 */
export interface ResultTabProps {
  tab: ResultTab;
  isActive: boolean;
  onClick: () => void;
  onClose: () => void;
}

/**
 * Props for TabBar component
 */
export interface TabBarProps {
  tabs: ResultTab[];
  activeTabId: string | null;
  onTabClick: (tabId: string) => void;
  onTabClose: (tabId: string) => void;
  onNewTab?: () => void;
  maxTabs?: number;
}

/**
 * Props for ResultsContainer component (main orchestrator)
 */
export interface ResultsContainerProps {
  // No props needed - uses Zustand store directly
}
