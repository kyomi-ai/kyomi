# Leptos SQL Editor — Implementation Plan

**Status:** Awaiting approval
**Branch:** TBD (will branch from `main`)
**Date:** 2026-03-22

## Overview

Migrate the SQL Editor page from React to Leptos. This is Phase 6 of the Leptos migration
(per `LEPTOS_MIGRATION_DESIGN.md`). The SQL Editor is the second-most complex page after
Dashboards, featuring a code editor, schema browser, query history, tabbed results with
pagination, dry run validation, streaming execution, and chart generation.

**Key decision:** The kode-leptos code editor (`~/repos/kode/kode-leptos`) replaces Monaco.
It already supports SQL syntax highlighting via tree-sitter and is used in the dashboard
editor and chart builder.

---

## Critical Rules for ALL Tasks

Every task in this plan MUST follow these rules. No exceptions.

### Rule 1: Read the React source FIRST
Before writing ANY Leptos code, open the exact React file specified in the task.
Copy CSS classes verbatim. Match HTML structure node for node. Include ALL sections.
Do NOT approximate from memory. Do NOT simplify. Do NOT skip "minor" sections.

### Rule 2: Follow the design system
Read `docs/DESIGN_SYSTEM.md` before writing any UI component.
Use semantic color tokens only (bg-primary, text-muted-foreground, etc.).
Use standard spacing scale (p-6 for cards, px-4 py-2 for buttons, etc.).
Use the Button, Card, Input, Label, Select components — do NOT create custom styles.

### Rule 3: Check for existing crates and components
Before building anything, check if a Rust crate exists (e.g., icondata_lu for icons).
Check `crates/kyomi-ui/src/components/` for existing shared components.
Reuse — do NOT duplicate.

### Rule 4: Register server functions
Every new `#[server(prefix = "/leptos-api")]` function needs a corresponding
`register_explicit::<TypeName>()` call in `crates/kyomi-ui/src/lib.rs`.

### Rule 5: Update the router
When the SQL Editor page is complete, update `app.rs` so `/sql-editor` routes to the
Leptos SQL Editor page instead of `NotImplementedPage`.

### Rule 6: Build and verify
After every change:
1. `cargo check -p kyomi-ui --features ssr` — server compiles
2. `cd crates/kyomi-ui && trunk build --public-url /leptos/` — WASM compiles
3. `touch apps/server/src/leptos_frontend.rs && cargo build -p kyomi-server` — server embeds new WASM

### Rule 7: Quality over speed
There is no deadline. It is more important to get it right than to get it done quickly.
Read the React source. Match it exactly. Review your own work before calling it done.

---

## Architecture Mapping: React → Leptos

### Component Mapping

| React Component | Leptos Component | Notes |
|----------------|-----------------|-------|
| `SQLEditorPage.jsx` | `pages/sql_editor/mod.rs` | Page shell with header, datasource selector |
| `SQLEditor.jsx` | `pages/sql_editor/sql_editor.rs` | Main orchestrator (sidebar + editor + results) |
| `MonacoSQLEditor.jsx` | kode-leptos `CodeEditor` | Direct replacement — Language::Sql |
| `DatasourceCatalogTree.jsx` | `pages/sql_editor/catalog_tree.rs` | Schema browser tree |
| `QueryHistorySidebar.jsx` | `pages/sql_editor/query_history.rs` | History list with infinite scroll |
| `ResultsContainer.jsx` | `pages/sql_editor/results_container.rs` | Tabbed results orchestrator |
| `TabBar.jsx` + `ResultTab.jsx` | `pages/sql_editor/tab_bar.rs` | Tab management |
| `ResultsTable.jsx` + `ResizableTable.jsx` | `pages/sql_editor/results_table.rs` | Data grid with pagination |
| `ResultsError.jsx` | Inline in results_container | Error display |
| `ResultsLoading.jsx` | Inline in results_container | Loading spinner |

### State Management Mapping

| React (Zustand) | Leptos | Notes |
|-----------------|--------|-------|
| `useSqlEditorStore` | `SqlEditorState` struct + signals | Reactive signals with localStorage persistence |
| `tabs: ResultTab[]` | `RwSignal<Vec<ResultTab>>` | Reactive tab list |
| `activeTabId` | `RwSignal<Option<String>>` | Reactive active tab |
| `queryText` | `RwSignal<String>` | Synced with kode-leptos `content` signal |
| `tableUIState` | `RwSignal<HashMap<String, TableUIState>>` | Per-tab pagination state |
| `defaultPageSize` | `RwSignal<u32>` | Persisted preference |
| `activeRightTab` | `RwSignal<Option<SidebarTab>>` | Enum: Catalog, History |
| localStorage persistence | `web_sys::window().local_storage()` | Manual ser/deser with serde_json |

### API Mapping

| React API Call | Leptos Server Function | Notes |
|---------------|----------------------|-------|
| `POST /api/v1/datasources/query/execute` | `execute_sql_query()` | Unified query execution |
| `POST /api/v1/datasources/query/execute?dry_run=true` | `dry_run_sql()` | Syntax validation |
| `POST /api/v1/datasources/query/stream` | `start_query_stream()` | Initiates streaming, results via WebSocket |
| `GET /api/v1/datasources/{slug}/catalog/tree` | `get_catalog_tree()` | Schema browser data |
| `POST /api/v1/bigquery/search` | `search_catalog()` | Semantic table search |
| `GET /api/v1/sql/history` | `list_query_history()` | History with search + pagination |
| `POST /api/v1/sql/history` | `save_query_history()` | Record executed query |
| `PATCH /api/v1/sql/history/{id}` | `update_query_history()` | Toggle saved status |
| `DELETE /api/v1/sql/history/{id}` | `delete_query_history()` | Delete from history |
| `POST /api/v1/datasources/{slug}/catalog/refresh` | `refresh_catalog()` | Trigger catalog refresh |
| `POST /api/v1/chart/generate` | `generate_chart()` | AI chart from results |
| `GET /api/v1/tables/info` | `get_table_info()` | Table detail for catalog |

---

## Phase 1: SQL Editor State & Server Functions

Build the state management layer and all server functions. No UI yet — this is the
foundation everything else depends on.

### Task 1.1: SQL Editor types and state management
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/types.rs`
**React reference:** `apps/frontend/src/features/sql-editor/types.ts`
**Read the React file. Match every type.**

Define all shared types that cross the server/client boundary:

```rust
// Query result types
pub struct ColumnMetadata { name: String, col_type: Option<String>, mode: Option<String> }
pub struct QueryHandle { datasource_type: String, datasource_slug: String, sql: String, job_id: Option<String> }
pub struct QueryResult { columns: Vec<ColumnMetadata>, rows: Vec<Vec<serde_json::Value>>, row_count: usize, total_rows: Option<usize>, query_handle: Option<QueryHandle>, execution_time_ms: Option<u64>, bytes_processed: Option<u64> }
pub struct QueryError { message: String, code: Option<String>, line: Option<u32>, column: Option<u32> }

// Tab types
pub enum QueryStatus { Idle, Running, Streaming, Success, Error }
pub struct ResultTab { id: String, label: String, query: String, status: QueryStatus, result: Option<QueryResult>, error: Option<QueryError>, pinned: bool, color_index: u8, created_at: f64, updated_at: f64, needs_refresh: bool, datasource_slug: Option<String>, datasource_type: Option<String> }

// UI state types
pub struct TableUIState { current_page: u32, page_size: u32 }
pub enum SidebarTab { Catalog, History }

// Catalog types
pub struct CatalogNode { name: String, node_type: CatalogNodeType, children: Vec<CatalogNode>, full_name: Option<String> }
pub enum CatalogNodeType { Project, Dataset, Schema, Database, Table, View, Column(String) }

// History types
pub struct QueryHistoryEntry { id: String, query_text: String, execution_time_ms: Option<i32>, bytes_processed: Option<i64>, row_count: Option<i32>, status: String, error_message: Option<String>, datasource: Option<String>, is_saved: bool, created_at: String }
```

All types must be `Clone + Serialize + Deserialize`.

**Acceptance criteria:**
- All types compile with both `ssr` and `hydrate` features
- Types match React's type definitions exactly in shape and semantics

### Task 1.2: SQL Editor state management (signals + localStorage)
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/state.rs`
**React reference:** `apps/frontend/src/features/sql-editor/store.ts` (240 lines)
**Read the entire React store. Match every action.**

Implement the SQL Editor state as a Leptos context provider with reactive signals:

```rust
pub struct SqlEditorState {
    pub tabs: RwSignal<Vec<ResultTab>>,
    pub active_tab_id: RwSignal<Option<String>>,
    pub query_text: RwSignal<String>,
    pub table_ui_state: RwSignal<HashMap<String, TableUIState>>,
    pub next_color_index: RwSignal<u8>,
    pub default_page_size: RwSignal<u32>,
    pub active_right_tab: RwSignal<Option<SidebarTab>>,
    pub sidebar_width: RwSignal<u32>,  // pixels, clamped 280-480
}
```

Methods to implement (matching Zustand actions exactly):
- `add_tab()` → creates tab, enforces 5-unpinned limit (removes oldest), cycles color 0-7
- `update_tab()` → updates tab properties
- `remove_tab()` → removes tab, auto-selects adjacent
- `set_active_tab()` → switches active tab
- `toggle_pin()` → pins/unpins tab
- `set_query_text()` → updates editor text
- `set_table_ui_state()` → updates per-tab pagination
- `set_default_page_size()` → user preference
- `clear_all_tabs()` → reset all

localStorage persistence (WASM-only):
- Save on every state change (debounced)
- On load: restore tabs (strip row data, mark `needs_refresh`), query text, preferences
- Use `web_sys::window().local_storage()` gated behind `#[cfg(target_arch = "wasm32")]`

Provide via Leptos context at the SQL Editor page level.

**Acceptance criteria:**
- State struct compiles and all actions work correctly
- localStorage round-trip: save → reload → state matches (minus row data)
- 5-unpinned-tab eviction works identically to React

### Task 1.3: Query execution server functions
**Creates:** `crates/kyomi-ui/src/server_fns/sql_editor.rs`
**React reference:** `apps/frontend/src/services/queryService.js` (137 lines),
  `apps/frontend/src/services/adapters/backendAdapter.js` (188 lines)
**Also read:** `apps/server/src/routes/datasources.rs` lines 2027-2200 (execute_query handler)

Server functions for query execution:

```rust
#[server(prefix = "/leptos-api")]
pub async fn execute_sql_query(
    datasource_slug: String,
    sql: String,
    page_size: u32,
    page: u32,
) -> Result<QueryResult, ServerFnError>
```

This server function replaces the REST `POST /api/v1/datasources/query/execute` endpoint.
It must:
1. Authenticate the user (extract_auth)
2. Resolve the datasource from slug
3. Build a provider via `kyomi_datasource_server`
4. Execute the query with LIMIT/OFFSET pagination
5. Return typed `QueryResult`

Also implement:

```rust
#[server(prefix = "/leptos-api")]
pub async fn fetch_query_page(
    datasource_slug: String,
    sql: String,
    page: u32,
    page_size: u32,
    job_id: Option<String>,
) -> Result<QueryResult, ServerFnError>
```

For BigQuery, `job_id` enables instant random page access. For other datasources,
re-executes with LIMIT/OFFSET.

**Acceptance criteria:**
- Server functions compile and are registered in lib.rs
- Query execution works for at least one datasource type (test manually or with unit test)
- Pagination returns correct page of results

### Task 1.4: Dry run server function
**Creates:** Add to `crates/kyomi-ui/src/server_fns/sql_editor.rs`
**React reference:** `apps/frontend/src/hooks/useSQLDryRun.js`,
  `apps/server/src/routes/datasources.rs` (dry_run path),
  `apps/server/src/routes/bigquery.rs` (bigquery_dry_run_cost)

```rust
#[server(prefix = "/leptos-api")]
pub async fn dry_run_sql(
    datasource_slug: String,
    sql: String,
) -> Result<DryRunResult, ServerFnError>

pub struct DryRunResult {
    pub valid: bool,
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub bytes_processed: Option<u64>,  // BigQuery cost estimation
}
```

The dry run validates SQL syntax without executing. For BigQuery it also returns
estimated bytes processed (cost estimation). For other datasources it uses
the provider's `dry_run()` method (EXPLAIN-based validation).

**Acceptance criteria:**
- Valid SQL returns `valid: true` with message
- Invalid SQL returns `valid: false` with error location (line/column)
- BigQuery dry run includes bytes_processed estimate

### Task 1.5: Query streaming server function
**Creates:** Add to `crates/kyomi-ui/src/server_fns/sql_editor.rs`
**React reference:** `apps/frontend/src/hooks/useQueryStream.js`,
  `apps/server/src/routes/datasources.rs` lines 2261-2444 (execute_query_stream)

```rust
#[server(prefix = "/leptos-api")]
pub async fn start_query_stream(
    datasource_slug: String,
    sql: String,
    request_id: String,
) -> Result<StreamStartResult, ServerFnError>

pub struct StreamStartResult {
    pub status: String,  // "streaming"
    pub request_id: String,
}
```

This initiates a streaming query. Results arrive via WebSocket events
(`query_stream_header`, `query_stream_chunk`, `query_stream_complete`,
`query_stream_error`) — the same WebSocket channel already used by the dashboard
live updates. The server function just kicks off the streaming job; the client
listens on the existing WebSocket connection.

**Acceptance criteria:**
- Server function initiates streaming and returns immediately
- WebSocket events are emitted to the user's channel
- Matches the exact same event format as the React streaming implementation

### Task 1.6: SQL history server functions
**Creates:** Add to `crates/kyomi-ui/src/server_fns/sql_editor.rs`
**React reference:** `apps/frontend/src/components/QueryHistorySidebar.jsx` (API calls),
  `apps/server/src/routes/sql_history.rs` (full route implementation)
**Read the sql_history.rs route file completely.**

```rust
#[server(prefix = "/leptos-api")]
pub async fn list_query_history(
    search: Option<String>,
    limit: u32,
    offset: u32,
) -> Result<Vec<QueryHistoryEntry>, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn save_query_history(
    query_text: String,
    execution_time_ms: Option<i32>,
    bytes_processed: Option<i64>,
    row_count: Option<i32>,
    status: String,
    error_message: Option<String>,
    datasource: Option<String>,
) -> Result<String, ServerFnError>  // returns query_id

#[server(prefix = "/leptos-api")]
pub async fn update_query_history(
    query_id: String,
    is_saved: Option<bool>,
) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn delete_query_history(
    query_id: String,
) -> Result<(), ServerFnError>
```

These call the same service-layer code as `apps/server/src/routes/sql_history.rs`.

**Acceptance criteria:**
- All 4 server functions compile and are registered
- list_query_history supports search filtering and pagination
- save_query_history records all metadata fields
- update_query_history toggles is_saved flag
- delete_query_history removes the entry

### Task 1.7: Catalog server functions
**Creates:** Add to `crates/kyomi-ui/src/server_fns/sql_editor.rs`
**React reference:** `apps/frontend/src/components/DatasourceCatalogTree.jsx` (API calls),
  `apps/server/src/routes/catalog.rs` (catalog tree endpoint)

```rust
#[server(prefix = "/leptos-api")]
pub async fn get_catalog_tree(
    datasource_slug: String,
    include_columns: bool,
) -> Result<Vec<CatalogNode>, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn search_catalog(
    datasource_slug: String,
    query: String,
) -> Result<Vec<CatalogNode>, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn refresh_catalog(
    datasource_slug: String,
) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn get_table_info(
    table_id: String,
) -> Result<serde_json::Value, ServerFnError>
```

The catalog tree varies by datasource type:
- BigQuery: project > dataset > table > column
- PostgreSQL: schema > table > column
- ClickHouse: database > table > column

The `CatalogNode` type's hierarchical structure handles all variants.

**Acceptance criteria:**
- Catalog tree returns correct hierarchy for at least BigQuery and PostgreSQL
- Search filters tables by name match
- Table info returns column details and metadata
- Refresh triggers a background catalog rebuild

### Task 1.8: Chart generation server function
**Creates:** Add to `crates/kyomi-ui/src/server_fns/sql_editor.rs`
**React reference:** `apps/frontend/src/features/sql-editor/components/ResultsContainer.jsx`
  (handleCreateChart function, calls `POST /api/v1/chart/generate`)

```rust
#[server(prefix = "/leptos-api")]
pub async fn generate_chart_from_results(
    columns: Vec<ColumnMetadata>,
    sample_rows: Vec<Vec<serde_json::Value>>,
    sql: String,
) -> Result<GeneratedChart, ServerFnError>

pub struct GeneratedChart {
    pub chartml_yaml: String,
    pub title: Option<String>,
}
```

This calls the AI chart generation service. Sends column metadata + sample rows,
gets back ChartML YAML.

**Acceptance criteria:**
- Server function compiles and is registered
- Returns valid ChartML YAML for a query result set

---

## Phase 2: SQL Code Editor Component

Build the SQL editor wrapper around kode-leptos with all the additional features
that the React MonacoSQLEditor provides.

### Task 2.1: SQL Editor wrapper component
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/code_editor.rs`
**React reference:** `apps/frontend/src/components/MonacoSQLEditor.jsx` (443 lines)
**Read the entire React file.**

Wrap kode-leptos `CodeEditor` with SQL-editor-specific features:

```rust
#[component]
pub fn SqlCodeEditor(
    content: Signal<String>,
    on_change: Arc<dyn Fn(String) + Send + Sync>,
    #[prop(optional)] on_run: Option<Arc<dyn Fn() + Send + Sync>>,
    #[prop(optional)] dry_run_result: Option<Signal<Option<DryRunResult>>>,
) -> impl IntoView
```

Features:
- kode-leptos `CodeEditor` with `Language::Sql`
- **Keyboard shortcut: Cmd/Ctrl+Enter** to run query (calls `on_run`)
- **Cursor position display** — show line:column in bottom-right status area
- **Error markers** — when `dry_run_result` has errors, display in status bar
  (kode-leptos doesn't have red squiggly underlines like Monaco, so show error
  location as text: "Error at line 3, column 12: ...")
- **WASM-only rendering** with SSR placeholder (same pattern as dashboard editor)

Note: kode-leptos does NOT support:
- Placeholder text (the React Monaco has "Enter your SQL query here...")
- Error squiggly underlines
- Suggestions/autocomplete

These are acceptable trade-offs. The status bar error display replaces squiggly
underlines, and the catalog sidebar replaces autocomplete.

**Acceptance criteria:**
- SQL syntax highlighting works correctly
- Cmd/Ctrl+Enter triggers the run callback
- Cursor position updates in real-time as user types/navigates
- Error location from dry run is displayed clearly
- Component renders loading placeholder during SSR

### Task 2.2: Dry run integration
**Creates:** Add to `crates/kyomi-ui/src/pages/sql_editor/code_editor.rs` or separate `dry_run.rs`
**React reference:** `apps/frontend/src/hooks/useSQLDryRun.js`
**Read the entire React hook.**

Implement debounced dry run validation as a Leptos effect:

- Watch the `query_text` signal
- After 1 second of no changes, call `dry_run_sql()` server function
- Update a `dry_run_result: RwSignal<Option<DryRunResult>>` signal
- Show validation status in the editor status bar:
  - Validating... (spinner)
  - Valid ✓ (green) + bytes estimate for BigQuery
  - Error at line X, column Y: message (red)

The debounce must cancel pending requests when the user types again.
Use `set_timeout` + `clear_timeout` via `web_sys` for the debounce.

**Acceptance criteria:**
- Dry run fires 1 second after user stops typing
- Previous pending dry run is cancelled when user types again
- Valid queries show green checkmark
- Invalid queries show error with line/column
- BigQuery queries show estimated bytes processed
- Dry run only fires when a datasource is selected

### Task 2.3: Status bar component
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/status_bar.rs`
**React reference:** `apps/frontend/src/components/SQLEditor.jsx` (status bar section at bottom of editor)

The status bar sits between the editor and the results panel. It shows:
- Left side: dry run status (validating/valid/error with message)
- Right side: cursor position (Ln X, Col Y)

Match the React styling exactly — read the SQLEditor.jsx for the status bar's
CSS classes and layout.

**Acceptance criteria:**
- Status bar matches React layout and styling
- Dry run status updates reactively
- Cursor position updates in real-time

---

## Phase 3: Sidebar — Catalog Tree & Query History

### Task 3.1: Sidebar shell (tabs + resize)
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/sidebar.rs`
**React reference:** `apps/frontend/src/components/SQLEditor.jsx` (sidebar section, lines ~100-200)
**Read the sidebar rendering and resize logic in SQLEditor.jsx.**

The right sidebar has:
- Two tabs: "Catalog" and "History"
- Resizable width via drag handle (pixel-based, clamped 280-480px)
- Toggle button to show/hide
- On mobile (<768px): slide-in overlay instead of side panel

The sidebar visibility and width are persisted in SqlEditorState.

Implement the resize handle using `mousedown` + `mousemove` + `mouseup` events
via `web_sys` (same pattern as the dashboard history panel resize).

**Acceptance criteria:**
- Tab switching between Catalog and History works
- Resize handle drags smoothly, width clamps to 280-480px
- Toggle button shows/hides sidebar
- Width persists across navigation
- Mobile: renders as overlay

### Task 3.2: Datasource Catalog Tree
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/catalog_tree.rs`
**React reference:** `apps/frontend/src/components/DatasourceCatalogTree.jsx`
**Read the entire React file.**

Hierarchical tree browser for database schemas:
- Fetches tree from `get_catalog_tree()` server function
- Expandable/collapsible nodes with icons per type (project, dataset/schema, table, column)
- **Click table** → inserts fully-qualified table name into editor at cursor
- **Click column** → inserts column name at cursor with leading comma + newline
- **Info button** on tables → calls `get_table_info()` and shows detail modal
- **Search** — text input at top filters visible nodes (700ms debounce)
- **Refresh button** — calls `refresh_catalog()`, shows spinner while refreshing
- **Loading skeleton** while initial fetch is in progress

For the "insert into editor" feature:
- kode-leptos doesn't expose an `insertTextAtCursor` imperative method
- Instead, the insert callback will modify the `content` signal directly by
  appending the text. This is a simpler approach that works for table/column insertion.

**OR** if kode-leptos adds cursor position access, we can insert at cursor position.
Check the kode-core `Editor` API — it has `cursor()` and `insert()` methods.
If accessible from the Leptos wrapper, use those for precise insertion.

**Acceptance criteria:**
- Tree renders correct hierarchy for the selected datasource
- Expanding/collapsing works for all node levels
- Click table inserts table name into editor
- Click column inserts column name
- Search filters visible nodes (debounced)
- Refresh button triggers catalog rebuild
- Info button opens table detail
- Loading skeleton shows during fetch
- Icons match React exactly (use icondata_lu)

### Task 3.3: Query History Sidebar
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/query_history.rs`
**React reference:** `apps/frontend/src/components/QueryHistorySidebar.jsx`
**Read the entire React file.**

Query history list with infinite scroll:
- Fetches from `list_query_history()` server function
- 50 items per page, loads more on scroll to bottom
- **Click query** → loads query text into editor, restores datasource selection
- **Star icon** → toggles `is_saved` via `update_query_history()`
- **Delete button** → removes via `delete_query_history()`, with confirmation
- **Search** — debounced 300ms text input filters results
- Shows: query text (truncated), datasource, timestamp, execution time, status icon
- Error queries shown with red indicator
- Saved queries shown with filled star

Infinite scroll: use an `IntersectionObserver` via `web_sys` on a sentinel element
at the bottom of the list. When it enters viewport, fetch next page.

**Acceptance criteria:**
- History list renders with all metadata fields
- Infinite scroll loads additional pages
- Click loads query into editor and selects correct datasource
- Star toggles saved status (optimistic update)
- Delete removes entry (with confirm dialog)
- Search filters results (300ms debounce)
- Timestamps formatted as relative time ("2 hours ago", "yesterday")
- Empty state shown when no history exists

---

## Phase 4: Results Panel — Tabs & Table

### Task 4.1: Tab bar component
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/tab_bar.rs`
**React reference:** `apps/frontend/src/features/sql-editor/components/TabBar.jsx` (122 lines)
  AND `apps/frontend/src/features/sql-editor/components/ResultTab.jsx` (227 lines)
**Read BOTH React files.**

Horizontal tab bar with:
- Scrollable tab area (horizontal overflow with hidden scrollbar)
- Each tab shows:
  - Colored circle (from chart palette, color_index 0-7)
  - Truncated query text as label
  - Row count + execution time
  - Status icon: spinner (running), red dot (error), nothing (success/idle)
  - Close button (×)
  - Pin button (pinned tabs bypass 5-tab limit)
- **Double-click tab** → restores query text to editor + selects datasource
- Active tab has highlighted background
- Smooth horizontal scrolling when many tabs exist

**Acceptance criteria:**
- Tab colors match React chart palette exactly
- Tab info (rows, time) displays correctly
- Status icons render for all states
- Close removes tab, auto-selects adjacent
- Pin/unpin works, pinned tabs show pin icon
- Double-click restores query to editor
- Scroll works when tabs overflow
- Matches React styling exactly

### Task 4.2: Results table with resizable columns
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/results_table.rs`
**React reference:** `apps/frontend/src/components/ResizableTable.jsx` (~350 lines)
  AND `apps/frontend/src/components/ResizableTable.css`
  AND `apps/frontend/src/features/sql-editor/components/ResultsTable.jsx` (100 lines)
**Read ALL THREE React files.**

Native HTML table with:
- **Fixed sticky header** — header row stays visible when scrolling vertically
- **Resizable columns** — drag handles between column headers
  - Track column widths in a signal (`Vec<f64>`)
  - Mousedown on handle starts resize, mousemove updates width, mouseup stops
  - Minimum column width: 50px
- **Scrollable body** — horizontal + vertical scroll
- **Text selection** — users can select cell text (no `user-select: none` on cells)
- **Row striping** — alternating row background colors
- **Cell rendering** — handle null values (show italic "null"), numbers right-aligned,
  dates formatted, long text truncated with tooltip

Match the CSS from `ResizableTable.css` exactly.

**Acceptance criteria:**
- Table renders column headers and rows correctly
- Header stays fixed when scrolling vertically
- Columns resize via drag handles
- Text is selectable in cells
- Null values displayed as italic "null"
- Numbers right-aligned
- Matches React styling exactly (copy CSS classes verbatim)

### Task 4.3: Pagination controls
**Creates:** Add to `results_table.rs` or separate `pagination.rs`
**React reference:** `apps/frontend/src/components/ResizableTable.jsx` (pagination section)

Server-side pagination controls below the table:
- Page numbers with prev/next arrows
- Page size dropdown (10, 25, 50, 100, 250, 500)
- "Showing X-Y of Z rows" text
- Changing page calls `fetch_query_page()` server function
- Changing page size re-executes the query with new size
- Loading state while fetching pages

**Acceptance criteria:**
- Page navigation works, fetches correct page from server
- Page size change re-executes query
- Row count display is accurate
- Loading spinner shown during page fetch
- Default page size comes from user preference (SqlEditorState)

### Task 4.4: Results container (orchestrator)
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/results_container.rs`
**React reference:** `apps/frontend/src/features/sql-editor/components/ResultsContainer.jsx` (567 lines)
**Read the entire React file.**

Main orchestrator for the results panel. Composes TabBar, ResultsTable, error/loading states:

- **Tab bar** at top
- **Active tab content** below:
  - Loading state → spinning Kyomi logo + message
  - Error state → error message + "Re-run Query" button for expired results
  - Success state → ResultsTable with data
  - Idle state → empty state message
- **Resizable split** — editor/results split via drag handle (vertical)
  - Split percentage stored in signal (clamped 20-80%)
  - Drag handle between editor and results panels
- **Auto-refresh** for tabs restored from localStorage (`needs_refresh` flag)
  - On mount, tabs with `needs_refresh: true` automatically re-fetch using their `query_handle`
- **Chart generation** — "Create Chart" button in table header
  - Calls `generate_chart_from_results()` server function
  - Shows ChartML preview using chartml-rs

**Error states (from React `ResultsError.jsx`):**
- Expired results: "Results have expired. Re-run this query to see the results."
  + Re-run button
- SQL error: formatted error message from `QueryError`
- Format BigQuery errors: extract meaningful text from "400 POST Syntax error" format,
  append line/column info, truncate > 200 chars

**Acceptance criteria:**
- Tab switching shows correct content for each tab
- Loading, error, success, idle states all render correctly
- Editor/results split resizes via drag handle
- Auto-refresh works for restored tabs
- Re-run button re-executes the original query
- Error formatting matches React's `formatSQLError` utility
- Chart generation button triggers AI chart creation

---

## Phase 5: Query Execution Flow

Wire everything together — the full query execution lifecycle from
pressing Cmd+Enter to seeing results in a tab.

### Task 5.1: Query execution handler
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/execution.rs`
**React reference:** `apps/frontend/src/components/SQLEditor.jsx` (handleRunQuery function)
  AND `apps/frontend/src/pages/SQLEditorPage.jsx` (onRunQuery)

The query execution flow:
1. User presses Cmd/Ctrl+Enter (or clicks Run button)
2. Get query text from editor (full text or selected text)
3. Validate: datasource must be selected, query must not be empty
4. Create a new tab in `Running` state via `state.add_tab()`
5. Set the new tab as active
6. Call `execute_sql_query()` server function
7. On success: update tab to `Success` state with result data
8. On error: update tab to `Error` state with error info
9. Fire-and-forget: call `save_query_history()` to record the execution

For streaming queries (when supported by datasource):
1. Same steps 1-4
2. Call `start_query_stream()` server function
3. Listen for WebSocket events (`query_stream_header`, `query_stream_chunk`, etc.)
4. Progressively update tab with incoming rows
5. On `query_stream_complete`: finalize tab as `Success`
6. On `query_stream_error`: update tab to `Error`

The decision of streaming vs non-streaming should match the React logic:
check if the datasource supports streaming (backend adapter datasources).

**Acceptance criteria:**
- Full query execution lifecycle works end-to-end
- New tab created in running state, updated on completion
- Error handling: network errors, SQL errors, timeout
- Query history saved after execution
- Streaming execution works via WebSocket
- Selected text execution (run highlighted portion only)

### Task 5.2: WebSocket streaming integration
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/streaming.rs`
**React reference:** `apps/frontend/src/hooks/useQueryStream.js`
**Also reference:** `crates/kyomi-ui/src/utils/websocket.rs` (existing WebSocket hook)

Extend the existing WebSocket infrastructure to handle query streaming events.
The dashboard already has `use_dashboard_updates()` in `utils/websocket.rs` —
follow the same pattern.

Events to handle:
- `query_stream_header` → `{ request_id, columns: [...] }` — set columns on tab
- `query_stream_chunk` → `{ request_id, rows: [[...], ...] }` — append rows to tab
- `query_stream_complete` → `{ request_id, total_rows, execution_time_ms }` — finalize
- `query_stream_error` → `{ request_id, error: "..." }` — set error on tab

Must match events by `request_id` to the correct tab.

**Acceptance criteria:**
- WebSocket events correctly update the matching tab
- Rows accumulate progressively during streaming
- Complete event finalizes the tab
- Error event sets error state
- Multiple concurrent streams work (different request_ids)

### Task 5.3: Datasource selector component
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/datasource_selector.rs`
**React reference:** `apps/frontend/src/pages/SQLEditorPage.jsx` (datasource selector in header)

Dropdown selector for choosing which datasource to query:
- Lists all active datasources from `list_datasources()` (already exists in server_fns/datasources.rs)
- Shows datasource name + type icon
- Selected datasource persisted to localStorage
- When changed: triggers dry run re-validation
- Empty state: "No datasources configured" with link to settings
- Groups by datasource type (optional, matches React if it does this)

**Acceptance criteria:**
- All active datasources listed
- Selection persists across page navigation
- Changing datasource triggers dry run
- Empty state shown when no datasources exist
- Matches React styling and layout

---

## Phase 6: Page Assembly & Integration

### Task 6.1: SQL Editor page shell
**Creates:** `crates/kyomi-ui/src/pages/sql_editor/mod.rs`
**React reference:** `apps/frontend/src/pages/SQLEditorPage.jsx` (160 lines)
  AND `apps/frontend/src/components/SQLEditor.jsx` (overall layout)
**Read BOTH React files.**

Assemble the full page:

```
┌──────────────────────────────────────────────────────────┐
│  Header: Page title + Datasource Selector + Run Button   │
├──────────────────────────────┬───────────────────────────┤
│                              │  Sidebar (Catalog/History) │
│  SQL Code Editor             │  ┌─────────────────────┐  │
│  (kode-leptos)               │  │ Tab: Catalog|History│  │
│                              │  ├─────────────────────┤  │
│                              │  │ Search...           │  │
│──────── Status Bar ──────────│  │ Tree / List         │  │
│                              │  │                     │  │
│  Results Panel               │  │                     │  │
│  ┌─────────────────────────┐ │  │                     │  │
│  │ Tab1 | Tab2 | Tab3      │ │  │                     │  │
│  ├─────────────────────────┤ │  │                     │  │
│  │ Results Table / Error   │ │  │                     │  │
│  │ / Loading / Empty       │ │  │                     │  │
│  │                         │ │  │                     │  │
│  │ Pagination              │ │  │                     │  │
│  └─────────────────────────┘ │  └─────────────────────┘  │
└──────────────────────────────┴───────────────────────────┘
```

Layout (matching React exactly):
- Full-height flex column
- Header bar at top with title, datasource selector, run button, sidebar toggle
- Main content area is a horizontal flex:
  - Left: editor (top) + results (bottom) with resizable vertical split
  - Right: collapsible sidebar
- State provider wraps the entire page

Wire up all the pieces:
- `SqlEditorState` provided via context
- `SqlCodeEditor` reads/writes `query_text` signal
- `DatasourceSelector` in header
- `Sidebar` with `CatalogTree` and `QueryHistory`
- `ResultsContainer` with `TabBar` and `ResultsTable`
- `StatusBar` between editor and results
- Run button + Cmd+Enter both trigger execution
- Catalog/history clicks insert into editor

### Task 6.2: Route registration
**Modifies:** `crates/kyomi-ui/src/app.rs`

Update the router to serve the Leptos SQL Editor at `/sql-editor`:
- Replace `NotImplementedPage` with `SqlEditorPage`
- Ensure the route is wrapped in `Layout` (sidebar navigation)

**Acceptance criteria:**
- `/sql-editor` renders the full Leptos SQL Editor page
- Navigation from sidebar works
- Page loads with correct initial state (restored from localStorage)

### Task 6.3: Mobile responsive layout
**Modifies:** `crates/kyomi-ui/src/pages/sql_editor/mod.rs` + sub-components
**React reference:** `apps/frontend/src/components/SQLEditor.jsx` (mobile detection + rendering)

Mobile (<768px) adaptations:
- Sidebar renders as a slide-in overlay instead of side panel
- Editor and results stack vertically (no side-by-side)
- Tab bar scrolls horizontally
- Run button prominently visible
- Datasource selector in a more compact form

**Acceptance criteria:**
- Usable on mobile viewport widths
- Sidebar overlays content on mobile
- All features accessible on mobile
- No horizontal overflow/scroll issues

---

## Phase 7: Polish & Parity

### Task 7.1: Chart builder integration
**Creates:** Integrate existing `crates/kyomi-ui/src/components/dashboard/chart_builder.rs`
**React reference:** `apps/frontend/src/features/sql-editor/components/ResultsContainer.jsx`
  (handleCreateChart, chart builder modal)

The "Create Chart" button in the results table header:
1. Calls `generate_chart_from_results()` with columns + sample rows
2. Opens the ChartBuilder modal (already exists as a dashboard component)
3. Shows chart preview rendered via chartml-rs
4. User can edit the ChartML YAML
5. Save to dashboard option (new or existing)

The ChartBuilder component already exists and renders ChartML. The integration task is:
- Add the "Create Chart" button to the results table header
- Wire it to the AI generation server function
- Open the existing ChartBuilder with the generated YAML
- Add "Save to Dashboard" flow (create new / add to existing)

**Acceptance criteria:**
- Create Chart button visible in results table header
- AI generates valid ChartML from query results
- Chart renders in preview using chartml-rs
- User can edit the YAML and see live updates
- Save to dashboard works (both new and existing)

### Task 7.2: Keyboard shortcuts
**Modifies:** Various files
**React reference:** `apps/frontend/src/components/MonacoSQLEditor.jsx` (keyboard handling)

Ensure all keyboard shortcuts work:
- **Cmd/Ctrl+Enter** — Run query (already in Task 2.1)
- **Cmd/Ctrl+S** — No-op (prevent browser save dialog)
- **Escape** — Close sidebar if open
- **Tab** — Insert spaces (handled by kode-leptos)

Register keyboard handlers at the page level for shortcuts that aren't editor-specific.

**Acceptance criteria:**
- All keyboard shortcuts work correctly
- No browser default actions triggered (e.g., no save dialog on Cmd+S)
- Shortcuts work when focus is in editor

### Task 7.3: Empty states and edge cases
**Modifies:** Various files
**React reference:** Various React components (empty states)

Handle all empty/edge states:
- No datasources configured → show message with link to settings/datasources
- No query history → show "No queries yet" message
- No results → show "Run a query to see results" message
- Query running → disable run button, show spinner in tab
- Network error → show retry option
- Multiple rapid executions → tabs created correctly, no race conditions
- Very wide result sets → horizontal scroll works
- Very tall result sets → vertical scroll with sticky header works
- Very long query text in history → truncated with ellipsis

**Acceptance criteria:**
- Every empty/edge state has appropriate UI
- No broken states or console errors
- Matches React empty states exactly

### Task 7.4: Accessibility
**Modifies:** Various files

Ensure accessibility:
- All interactive elements have appropriate ARIA labels
- Tab navigation works (keyboard-only usage)
- Screen reader announces tab changes, loading states, errors
- Focus management: after query execution, focus moves to results
- Color contrast meets WCAG AA
- Resize handles accessible via keyboard

**Acceptance criteria:**
- Tab key navigates through all interactive elements
- Screen readers announce state changes
- No accessibility violations in automated audit

---

## Verification After Each Phase

After completing each phase:
1. All `cargo check -p kyomi-ui --features ssr` passes with zero errors
2. `cd crates/kyomi-ui && trunk build --public-url /leptos/` succeeds
3. Server starts and serves the Leptos page
4. Visual comparison: Leptos page matches React page in browser
5. All server functions return correct data
6. SQL editor route serves Leptos (after Phase 6)

---

## File Structure

```
crates/kyomi-ui/src/pages/sql_editor/
├── mod.rs                    # SqlEditorPage — page shell + assembly
├── types.rs                  # All shared types (ResultTab, QueryResult, etc.)
├── state.rs                  # SqlEditorState — signals + localStorage
├── code_editor.rs            # SQL editor wrapper around kode-leptos
├── status_bar.rs             # Dry run status + cursor position
├── sidebar.rs                # Sidebar shell (tabs + resize)
├── catalog_tree.rs           # Datasource catalog tree browser
├── query_history.rs          # Query history list with infinite scroll
├── results_container.rs      # Results orchestrator (tabs + table + states)
├── tab_bar.rs                # Tab bar + individual tab components
├── results_table.rs          # Resizable data table + pagination
├── datasource_selector.rs    # Datasource dropdown in header
├── execution.rs              # Query execution handler
└── streaming.rs              # WebSocket streaming integration

crates/kyomi-ui/src/server_fns/
└── sql_editor.rs             # All SQL editor server functions
```

---

## Summary

| Phase | Tasks | New Server Fns | New Components | Complexity |
|-------|-------|---------------|----------------|------------|
| 1 | 8 | ~12 (query, dry run, stream, history, catalog, chart) | 0 (types + state only) | Medium |
| 2 | 3 | 0 | SqlCodeEditor, DryRun integration, StatusBar | Medium |
| 3 | 3 | 0 | Sidebar, CatalogTree, QueryHistory | Medium-Hard |
| 4 | 4 | 0 | TabBar, ResultsTable, Pagination, ResultsContainer | Hard |
| 5 | 3 | 0 | Execution handler, Streaming, DatasourceSelector | Hard |
| 6 | 3 | 0 | Page assembly, Route, Mobile | Medium |
| 7 | 4 | 0 | Chart builder, Shortcuts, Edge cases, Accessibility | Medium |
| **Total** | **28** | **~12** | **~14** | |

### Dependencies Between Phases

```
Phase 1 (State + Server Fns)
  ├── Phase 2 (Code Editor) — needs types, state, dry run fn
  ├── Phase 3 (Sidebar) — needs catalog + history server fns, state
  └── Phase 4 (Results) — needs types, state, query result fns
        └── Phase 5 (Execution) — needs editor, results, state, all server fns
              └── Phase 6 (Assembly) — needs everything above
                    └── Phase 7 (Polish) — needs assembled page
```

Phases 2, 3, and 4 can be worked on in parallel after Phase 1 is complete.
Phase 5 requires 2+4. Phase 6 requires all of 2-5. Phase 7 is final polish.
