// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * SQL Editor Feature - Tabbed Results Architecture
 *
 * Export all components and utilities for the SQL editor feature.
 * See ARCHITECTURE.md for detailed documentation on component responsibilities.
 */

// Components
export { default as ResultsContainer } from './components/ResultsContainer.jsx';
export { default as ResultsTable } from './components/ResultsTable.jsx';
export { default as ResultsError } from './components/ResultsError.jsx';
export { default as ResultsLoading } from './components/ResultsLoading.jsx';
export { default as ResultTab } from './components/ResultTab.jsx';
export { default as TabBar } from './components/TabBar.jsx';

// Utilities
export { formatSQLError } from './utils/formatSQLError.js';

// Store and hooks
export {
  useSqlEditorStore,
  useActiveTab,
  useTableUIState,
  useIsTabActive,
} from './store';

// Types (for TypeScript/JSDoc users)
export * from './types';
