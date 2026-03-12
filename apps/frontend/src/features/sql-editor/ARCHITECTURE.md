# SQL Editor Architecture

## Overview

The SQL Editor feature allows users to write and execute SQL queries against any connected datasource (BigQuery, PostgreSQL, ClickHouse, Snowflake, etc.), view results in a tabbed interface, and create charts from query results.

## Component Hierarchy

```
pages/SQLEditorPage.jsx (Page wrapper - routing entry point)
└── components/SQLEditor.jsx (Main orchestrator)
    ├── components/MonacoSQLEditor.jsx (Monaco editor wrapper)
    ├── components/BigQueryCatalogTree.jsx (Schema browser sidebar)
    ├── components/QueryHistorySidebar.jsx (Query history sidebar)
    └── features/sql-editor/components/ResultsContainer.jsx (Results orchestrator)
        ├── TabBar.jsx (Tab management bar)
        │   └── ResultTab.jsx (Individual result tab)
        ├── ResultsTable.jsx (Data grid with pagination)
        ├── ResultsError.jsx (Error display with reconnect actions)
        └── ResultsLoading.jsx (Loading spinner)
```

## File Responsibilities

### Page Layer
| File | Lines | Responsibility |
|------|-------|----------------|
| `pages/SQLEditorPage.jsx` | ~140 | Route entry point. Sets up the page header, datasource selector, sidebar toggle, and wires up query execution handler via `queryService`. Saves queries to history. |

### Component Layer (`components/`)
| File | Lines | Responsibility |
|------|-------|----------------|
| `SQLEditor.jsx` | ~1039 | **Main orchestrator**. Contains: Monaco editor integration, sidebar management (catalog/history), dry-run validation display, query execution triggering, datasource status checks. Renders `ResultsContainer` for displaying results. |
| `MonacoSQLEditor.jsx` | ~406 | **Uncontrolled** Monaco wrapper. Handles: syntax highlighting, cursor position, keyboard shortcuts (Cmd+Enter), text selection. Exposes imperative API via `ref`: `getValue()`, `setValue()`, `getSelectedOrFullText()`. |

### Feature Layer (`features/sql-editor/`)
| File | Lines | Responsibility |
|------|-------|----------------|
| `ResultsContainer.jsx` | ~630 | Results display orchestrator. Manages tab state via Zustand, pagination via `queryService.fetchPage()`, chart generation, OAuth reconnection, and tab auto-refresh for restored sessions. |
| `TabBar.jsx` | ~118 | Horizontal tab bar. Renders tabs, handles click/close/pin actions. |
| `ResultTab.jsx` | ~227 | Individual tab component. Shows status (running/error/success), row count, execution time. Colored indicators for visual differentiation. |
| `ResultsTable.jsx` | ~98 | Thin wrapper around `ResizableTable`. Transforms query result format and passes pagination handlers. |
| `ResultsError.jsx` | ~209 | Error display with context-aware UI: OAuth errors show reconnect button, expired results show re-run button, SQL errors show formatted message. |
| `ResultsLoading.jsx` | ~32 | Simple loading spinner with animated Kyomi logo. |

### State Management (`features/sql-editor/`)
| File | Responsibility |
|------|----------------|
| `store.ts` | Zustand store for tabs, active tab, query text, table UI state. Persisted to localStorage. |
| `types.ts` | TypeScript interfaces for ResultTab, QueryResult, QueryHandle, TableUIState, etc. |
| `index.js` | Barrel exports for components, store hooks, and types. |

### Services Layer (`services/`)
| File | Responsibility |
|------|----------------|
| `queryService.js` | **Unified query execution interface**. Routes to appropriate adapter based on datasource type. Provides `executeQuery()`, `fetchPage()`, and `dryRun()` methods. |
| `adapters/bigQueryAdapter.js` | BigQuery-specific adapter. Uses direct REST API with OAuth. Supports random page access via `jobId`. |
| `adapters/backendAdapter.js` | Adapter for all non-BigQuery datasources. Routes through backend API with LIMIT/OFFSET pagination. |

## Data Flow

### Query Execution Flow
```
1. User types SQL in MonacoSQLEditor
2. SQLEditor.handleRunQuery() called (via button or Cmd+Enter)
3. SQLEditor creates new tab in Zustand store (status: 'running')
4. SQLEditorPage.handleRunQuery() executes via queryService.executeQuery()
5. queryService routes to appropriate adapter (BigQuery or backend)
6. Results returned → SQLEditor updates tab (status: 'success', result data with queryHandle)
7. ResultsContainer renders ResultsTable with pagination
8. SQLEditorPage saves query to history (fire-and-forget)
```

### Pagination Flow
```
1. User clicks page number in ResultsTable
2. ResultsContainer.handlePageChange() called with page number
3. queryService.fetchPage(queryHandle, page, pageSize) called
   - BigQuery: Uses jobId for instant random page access
   - Others: Re-executes query with LIMIT/OFFSET
4. Results returned → tab updated with new rows (preserving queryHandle)
```

### Dry Run Validation Flow
```
1. MonacoSQLEditor.onChange fires on every keystroke
2. SQLEditor.handleEditorChange() triggers dry run via useSQLDryRun hook
3. Hook debounces (800ms) and calls queryService.dryRun()
   - BigQuery: Direct API for cost estimate (bytes_processed, estimated_cost)
   - Others: Backend EXPLAIN for syntax validation
4. Result displayed in status bar; errors shown as editor markers
```

### Tab State Flow
```
┌─────────────────┐      ┌──────────────────┐      ┌────────────────┐
│  SQLEditor      │      │  Zustand Store   │      │ ResultsContainer│
│  (creates tabs) │─────▶│  (tabs, active)  │◀─────│ (reads tabs)   │
└─────────────────┘      └──────────────────┘      └────────────────┘
                                 │
                                 ▼
                         ┌──────────────┐
                         │ localStorage │
                         │ (persisted)  │
                         └──────────────┘
```

## Key Design Decisions

### 1. Uncontrolled Monaco Editor
`MonacoSQLEditor` is **uncontrolled** - it manages its own internal state. This prevents:
- Re-renders on every keystroke
- Cursor position jumping
- Performance issues with large queries

Access content via ref: `editorRef.current.getValue()`

### 2. Tab State in Zustand
Results are stored in a Zustand store rather than component state:
- Persistence across navigation (tabs survive route changes)
- Selective subscriptions prevent unnecessary re-renders
- Shared access between SQLEditor and ResultsContainer

### 3. Unified QueryService with Adapters
All query execution goes through `queryService` which routes to the appropriate adapter:
- **BigQuery**: Direct REST API (OAuth) for performance, `jobId`-based pagination
- **Others** (PostgreSQL, ClickHouse, etc.): Backend proxy with LIMIT/OFFSET pagination

This eliminates datasource-specific conditionals in UI components.

### 4. QueryHandle for Pagination
The `queryHandle` object contains all info needed for pagination:
```typescript
interface QueryHandle {
  datasourceType: string;   // 'bigquery', 'postgres', etc.
  datasourceSlug: string;   // 'production-postgres'
  sql: string;              // Original query (for re-execution)
  jobId?: string;           // BigQuery-specific: job ID for random page access
}
```

This is stored with each tab result and used by `queryService.fetchPage()`.

### 5. Component Split Rationale
- `SQLEditor.jsx` in `components/` - it's a reusable component (used in ChartBuilderModal too)
- `ResultsContainer` and friends in `features/sql-editor/` - tightly coupled to SQL editor feature

## "Where Do I Fix X?" Guide

| Bug Type | Look In |
|----------|---------|
| Editor cursor/selection issues | `MonacoSQLEditor.jsx` |
| Dry run validation issues | `SQLEditor.jsx`, `useSQLDryRun.js`, `queryService.js` |
| Tab display/styling | `ResultTab.jsx` |
| Tab bar layout/interactions | `TabBar.jsx` |
| Results table rendering | `ResultsTable.jsx`, `ResizableTable.jsx` |
| Pagination issues | `ResultsContainer.jsx` (handlers), `queryService.js` (routing), `adapters/*.js` (implementation) |
| Error message formatting | `ResultsError.jsx` |
| Query execution | `SQLEditorPage.jsx` (handler), `queryService.js` (routing), `adapters/*.js` (implementation) |
| Tab state/persistence | `features/sql-editor/store.ts` |
| Sidebar (catalog/history) | `SQLEditor.jsx` (container), `BigQueryCatalogTree.jsx`, `QueryHistorySidebar.jsx` |
| Datasource routing | `queryService.js`, `adapters/bigQueryAdapter.js`, `adapters/backendAdapter.js` |

## Shared Utilities

### `formatSQLError()` - SQL Error Formatting
**Location:** `features/sql-editor/utils/formatSQLError.js`

Used by both `SQLEditor.jsx` (dry run errors) and `ResultsError.jsx` (execution errors) to parse and clean SQL error messages.

---

*Last updated: 2025-12-20*
