# Unified Arrow Query Path — Implementation Plan

## Problem

Three separate data paths exist for querying datasources:
1. `query_datasource_arrow` — returns base64-encoded Arrow IPC via Leptos
   server function, used by chartml, caps at 10,000 rows
2. `execute_sql_query` — returns JSON rows via `record_batch_to_json_rows`,
   uses LIMIT/OFFSET pagination
3. BigQuery direct REST in `sql_editor.rs` (~400 lines) that bypasses the
   provider for job_id-based pagination

All paths call `provider.execute_query` (buffered, max 1000 rows default).
ChartML can't pull more than 10,000 rows — a regression from the DuckDB era.
In the DuckDB era, chartml streamed millions of rows into the browser, loaded
them into DuckDB, and ran aggregate queries client-side. The replacement is
DataFusion (`chartml-datafusion` / `chartml-wasm-datafusion`), which runs the
same 3-stage transform pipeline (SQL → Aggregate → Forecast) in WASM. But
the transport layer still caps at 10,000 rows, breaking this flow.

The SQL editor converts Arrow → JSON on the server.
The transport base64-encodes IPC bytes (33% overhead).
The BigQuery job_id logic lives in the server function layer instead of the
provider where it belongs.

Additionally, `execute_query_stream_arrow` overrides only exist for sqlx
providers (Postgres, MySQL, Redshift). The 4 REST providers (BigQuery,
ClickHouse, Snowflake, Databricks) and 2 tiberius providers (SQL Server,
Synapse) fall back to buffered `execute_query`, which OOMs on large datasets.

## Goal

One data path: Arrow IPC bytes from database to browser for both chartml
and SQL editor. Binary HTTP transport via a raw axum endpoint — no base64,
no JSON conversion, no Leptos server function. The only difference between
chartml and SQL editor is the presentation layer (DataFusion transform →
chart renderer vs results table). All 9 providers stream large datasets
without OOM. Server streams batches to the HTTP response body as they
arrive from the provider — no server-side buffering.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ Server                                                          │
│                                                                 │
│  POST /api/v1/query-arrow                                       │
│  Body: JSON { datasource_slug, sql, limit?, offset?,            │
│               include_total?, job_id? }                         │
│  Auth: session cookie (same as Leptos server fns)               │
│                                                                 │
│  ├─ Auth + resolve datasource + create provider                 │
│  ├─ Validate: limit/offset non-negative (400 if negative)       │
│  ├─ Internal dispatch (same endpoint, same wire format):        │
│  │                                                              │
│  │  IF limit is set (SQL editor paginated path):                │
│  │    provider.execute_query(sql, limit, offset, job_id)        │
│  │    → single RecordBatch → write one IPC batch to body        │
│  │    → preserves BigQuery job_id for page-to-page reuse        │
│  │                                                              │
│  │  IF no limit (chartml streaming path):                       │
│  │    provider.execute_query_stream_arrow(sql)                  │
│  │    → yields ArrowStreamEvent::Batch per page/chunk           │
│  │    → each batch written to HTTP body immediately             │
│  │    → no server-side buffering of full result set             │
│  │                                                              │
│  ├─ For Connect datasources: ConnectProvider proxies via        │
│  │   WebSocket, yields ArrowBatch messages as they arrive       │
│  └─ Response: Arrow IPC streaming format                        │
│     Content-Type: application/vnd.apache.arrow.stream           │
│     Headers: X-Total-Rows, X-Job-Id (when known before body)   │
│     Body: [schema msg][batch 1][batch 2]...[EOS marker]         │
│     Final batch schema metadata: execution_time_ms,             │
│       bytes_processed, has_more (known only after stream ends)  │
│                                                                 │
│  One endpoint. All providers. All datasource types.             │
│  Connect and direct datasources use the same code path.         │
└─────────────────────────┬───────────────────────────────────────┘
                          │ Raw Arrow IPC bytes (binary HTTP body)
                          │ gzip/brotli compressed by nginx
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ Browser (WASM)                                                  │
│                                                                 │
│  fetch("/api/v1/query-arrow", { method: "POST", body: ... })    │
│                                                                 │
│  SQL editor (paginated, ~100 rows):                             │
│    response.arrayBuffer() → DataTable::from_ipc_bytes()         │
│    → render results table                                       │
│    Read X-Total-Rows, X-Job-Id from response.headers            │
│                                                                 │
│  chartml (large datasets, millions of rows):                    │
│    response.body.getReader() → ReadableStream                   │
│    → decode Arrow IPC batches incrementally                     │
│    → feed each RecordBatch to TransformMiddleware               │
│    → transform: SQL → Aggregate → Forecast                      │
│    → small aggregated DataTable → chart renderer                │
│                                                                 │
│  Same endpoint, same wire format. Client chooses consumption    │
│  pattern based on use case.                                     │
│                                                                 │
│  IPC decoding uses chartml-core's arrow dep (works on wasm32).  │
│  kyomi-ui's arrow deps are SSR-only, NOT used in browser.       │
│  TransformMiddleware streaming input: separate chartml plan.    │
└─────────────────────────────────────────────────────────────────┘
```

## Transport details

See the Architecture section above for the high-level flow. Key details:

**Internal dispatch**: The endpoint routes based on request parameters:
- `limit` set → `execute_query` (buffered, one batch, job_id works)
- No `limit` → `execute_query_stream_arrow` (streaming, multiple batches)

Same endpoint, same wire format. The client doesn't know which trait
method was called.

**Response metadata**: Values known before the body (total_rows, job_id)
go in regular HTTP headers. Values known after the stream completes
(execution_time_ms, bytes_processed, has_more) go in Arrow schema
metadata on a final zero-row RecordBatch. Browsers can't read HTTP
trailer headers, so we use Arrow's built-in schema metadata extensibility.

**Error handling**: Pre-stream errors return proper HTTP status codes
(400/401/403/422/500) with JSON body. Mid-stream errors: server drops
the writer (truncates response), client detects missing EOS marker and
shows "Query interrupted."

**Schema consistency**: REST providers must capture the schema from page 1
and validate subsequent pages match. Schema change mid-stream → error.

**Cross-replica Connect**: Large streams through Redis pub/sub have no
backpressure. Known limitation. Same-pod routing (DashMap) avoids Redis.

**Compression**: nginx-level gzip. Add `application/vnd.apache.arrow.stream`
to `gzip_types`. Self-hosted users configure independently.

---

## Tasks

Tasks are grouped by repo. Within each repo, tasks are ordered by
dependency. Each task is one agent session.

### kyomi-connect repo tasks

#### Task 1: Provider trait — add job_id param + QueryResult field

Add `job_id: Option<&str>` to `execute_query` on the `DatasourceProvider`
trait and `job_id: Option<String>` to `QueryResult`. All 9 providers get
`_job_id: Option<&str>` param (ignored), return `job_id: None`. Update
all callers to pass `None`. Add doc comment explaining stateful pagination.

**Files:**
- `crates/kyomi-datasource/src/provider.rs` — trait + QueryResult
- `crates/kyomi-datasource/src/providers/postgres.rs` — add `_job_id` param
- `crates/kyomi-datasource/src/providers/mysql.rs` — add `_job_id` param
- `crates/kyomi-datasource/src/providers/redshift.rs` — add `_job_id` param
- `crates/kyomi-datasource/src/providers/bigquery.rs` — add `_job_id` param
- `crates/kyomi-datasource/src/providers/clickhouse.rs` — add `_job_id` param
- `crates/kyomi-datasource/src/providers/snowflake.rs` — add `_job_id` param
- `crates/kyomi-datasource/src/providers/databricks.rs` — add `_job_id` param
- `crates/kyomi-datasource/src/providers/sqlserver.rs` — add `_job_id` param
- `crates/kyomi-datasource/src/providers/synapse.rs` — add `_job_id` param
- Any other callers of `execute_query` in the crate

**Depends on:** nothing

#### Task 2: BigQuery job_id implementation

Implement actual job_id pass-through in the BigQuery provider. When
`job_id` is `Some`, skip `submit_query_job` and call `get_query_results`
directly with the given job_id + offset. When `None`, submit the job and
return `job_id: Some(id)` in the result.

**Files:**
- `crates/kyomi-datasource/src/providers/bigquery.rs`

**Depends on:** Task 1

#### Task 3: Wire protocol + executor job_id pass-through

Add `job_id: Option<String>` to `QueryParams` (with `#[serde(default)]`)
and to `ArrowComplete` metadata. Pass through in the executor.

**Files:**
- `crates/kyomi-connect-protocol/src/wire.rs` — QueryParams + ArrowComplete
- `crates/kyomi-connect/src/executor.rs` — pass job_id through

**Depends on:** Task 1

#### Task 4: BigQuery streaming override

Override `execute_query_stream_arrow` on the BigQuery provider. Submit
query job → paginate via `getQueryResults` with `startIndex` + `maxResults`
→ each page: JSON rows → `bigquery_row_to_arrow` → yield batch → continue
until exhausted. Enforce schema consistency across pages.

**Files:**
- `crates/kyomi-datasource/src/providers/bigquery.rs`

**Depends on:** Task 2

#### Task 5: ClickHouse streaming override

Override `execute_query_stream_arrow` on ClickHouse. Paginate via
LIMIT/OFFSET (same approach as `prepare_query` already uses). One HTTP
request per page → `clickhouse_row_to_arrow` → yield batch. Enforce
schema consistency across pages.

**Files:**
- `crates/kyomi-datasource/src/providers/clickhouse.rs`

**Depends on:** Task 1

#### Task 6: Snowflake streaming override

Override `execute_query_stream_arrow` on Snowflake. Submit query → get
statement handle → fetch partitions sequentially → `snowflake_row_to_arrow`
→ yield batch. Enforce schema consistency across partitions.

**Files:**
- `crates/kyomi-datasource/src/providers/snowflake.rs`

**Depends on:** Task 1

#### Task 7: Databricks streaming override

Override `execute_query_stream_arrow` on Databricks. Submit statement →
poll until SUCCEEDED → fetch chunks → `databricks_row_to_arrow` → yield
batch. Enforce schema consistency across chunks.

**Files:**
- `crates/kyomi-datasource/src/providers/databricks.rs`

**Depends on:** Task 1

#### Task 8: SQL Server + Synapse streaming override

Override `execute_query_stream_arrow` on SQL Server and Synapse using
tiberius streaming (shared `tsql_common` implementation). Holds mutex on
TDS client, yields rows into ArrowResultBuilder.

**Files:**
- `crates/kyomi-datasource/src/providers/sqlserver.rs`
- `crates/kyomi-datasource/src/providers/synapse.rs`
- `crates/kyomi-datasource/src/providers/tsql_common.rs` (if shared helper needed)

**Depends on:** Task 1

#### Task 9: Crate publish

After all streaming overrides are done (Tasks 4-8). Bump
kyomi-connect-protocol and kyomi-datasource to 1.2.0. Tag and push to
trigger crates.io publish.

**Depends on:** Tasks 4, 5, 6, 7, 8

---

### kyomi repo tasks

#### Task 10: ConnectProvider streaming override

Add `execute_query_stream_arrow` override to ConnectProvider that yields
each ArrowBatch WebSocket message as an `ArrowStreamEvent` instead of
collecting into one RecordBatch via `collect_arrow_stream()`. Remove
`collect_arrow_stream()` and the >1 batch rejection check.

**Files:**
- `crates/kyomi-datasource/src/connect/provider.rs`

**Depends on:** Task 3 (wire protocol changes)

#### Task 11: Arrow IPC HTTP endpoint

New `POST /api/v1/query-arrow` axum route. Handles auth, datasource
resolution, input validation (negative limit/offset → 400). Internal
dispatch: limit set → `execute_query` (buffered), no limit →
`execute_query_stream_arrow` (streaming via DuplexStream). Proper error
semantics (400/401/403/422/500). Metadata in HTTP headers (pre-stream)
and Arrow schema metadata on final zero-row batch (post-stream).
Mid-stream errors: drop writer, client detects truncation.

**Files:**
- `apps/server/src/routes/query_arrow.rs` (new)
- `apps/server/src/routes/mod.rs` — register route
- `apps/server/src/lib.rs` — add to axum router

**Depends on:** Task 10

#### Task 12: WASM fetch helpers

Two shared async functions for browser-side consumption of the Arrow
endpoint. Both use `fetch()` with `credentials: "same-origin"`. Handle
422 → parse JSON error body, 4xx/5xx → error, truncated stream →
"Query interrupted."

- `fetch_arrow_buffered` — `response.arrayBuffer()` → `DataTable::from_ipc_bytes()`,
  reads X-Total-Rows/X-Job-Id/X-Has-More from headers.
- `fetch_arrow_stream` — `response.body.getReader()` → yields RecordBatches
  incrementally. Caller feeds to TransformMiddleware.

Both `#[cfg(target_arch = "wasm32")]` only.

**Files:**
- `crates/kyomi-ui/src/helpers/arrow_fetch.rs` (new)

**Depends on:** Task 11

#### Task 13: chartml provider migration

Replace `query_datasource_arrow` server fn call with `fetch_arrow_stream`
from Task 12. Delete `build_fetch_result` base64 helper. Keep
`DatasourceQuerier` trait for testability.

**Files:**
- `crates/kyomi-ui/src/chartml_provider.rs`

**Depends on:** Task 12

#### Task 14: SQL editor migration

Replace `execute_sql_query` with `fetch_arrow_buffered`. Update types,
results table, state, and results container.

- `QueryResult` holds `DataTable` instead of `Vec<Vec<Value>>`, drop serde
- `ResultTab.result` gets `#[serde(skip)]`, restore shows "Results expired —
  click to re-run" (no auto-execute)
- Results table renders via `data.get_string(row_idx, col_name)`, numeric
  alignment via `is_numeric()`, null detection via `is_null()`
- Handle job_id from headers, pass on page fetches. Stale BigQuery job_id
  (422) → clear and re-execute transparently
- Handle 422 → display provider error message

**Files:**
- `crates/kyomi-ui/src/pages/sql_editor/execution.rs`
- `crates/kyomi-ui/src/pages/sql_editor/types.rs`
- `crates/kyomi-ui/src/pages/sql_editor/results_table.rs`
- `crates/kyomi-ui/src/pages/sql_editor/results_container.rs`
- `crates/kyomi-ui/src/pages/sql_editor/state.rs`

**Depends on:** Task 12

#### Task 15: Delete old query paths

Delete all old server functions, helpers, and dead code. Sweep
`register_explicit` in `lib.rs` for removed server fns.

**kyomi repo delete list:**
- `query_datasource_arrow` server fn
- `execute_sql_query` server fn
- `fetch_query_page` server fn
- `start_query_stream` server fn (dead, zero callers)
- All BigQuery direct REST code in `sql_editor.rs` (~400 lines)
- `record_batch_to_json_rows`
- `provider_result_to_query_result`
- `QueryArrowResult` type
- `format_cell` JSON formatter
- `ConnectProvider.execute_query_stream` (JSON streaming)
- `async_stream`, `map_response_to_event`, `collect_stream_to_result`,
  `query_result_to_stream`
- `collect_arrow_stream` (already replaced in Task 10)

Clean up `is_terminal` in registry.rs (verify no stale variants).

**Files:**
- `crates/kyomi-ui/src/server_fns/sql_editor.rs`
- `crates/kyomi-ui/src/server_fns/datasources.rs`
- `crates/kyomi-ui/src/lib.rs` (register_explicit sweep)
- `crates/kyomi-datasource/src/connect/provider.rs`
- `crates/kyomi-datasource/src/connect/registry.rs`
- `crates/kyomi-datasource/src/lib.rs`

**Depends on:** Tasks 13, 14 (all consumers migrated)

#### Task 16: kyomi-connect cleanup + dependency update

Remove dead fields from kyomi-connect crate. Update kyomi dependency
versions. Keep `query_result_to_arrow_stream` (still used for DDL/dry_run).

**kyomi-connect cleanup:**
- Remove `rows` field from `QueryResult`
- Remove `Serialize, Deserialize` from `QueryResult`
- Remove `serde_json::Value` import if unused

**kyomi dependency update:**
- `Cargo.toml` → `kyomi-connect-protocol = "1.2"`,
  `kyomi-datasource-drivers = { version = "1.2", ... }`
- `crates/kyomi-datasource/Cargo.toml` → add `arrow-select` for `concat_batches`

**Depends on:** Task 9 (crates published), Task 15

#### Task 17: nginx configuration

Add `application/vnd.apache.arrow.stream` to `gzip_types` on the NAS
nginx config.

**Files:**
- `/etc/nginx/nginx.conf` on 192.168.1.100

**Depends on:** Task 11 (endpoint exists)

---

### chartml repo tasks

#### Task 18: Time64 formatting in arrow_to_string

Add `Time64(Microsecond)` → HH:MM:SS formatting to `arrow_to_string`.
Closes the one gap between `DataTable.get_string()` and the current
`record_batch_to_json_rows`.

**Files:**
- `crates/chartml-core/src/data.rs`

**Depends on:** nothing (can run in parallel with everything)

---

## Task dependency graph

```
kyomi-connect repo:
  Task 1 (trait + job_id)
    ├─ Task 2 (BQ job_id impl) → Task 4 (BQ streaming)
    ├─ Task 3 (wire protocol)
    ├─ Task 5 (ClickHouse streaming)     ─┐
    ├─ Task 6 (Snowflake streaming)       ├─ Task 9 (publish)
    ├─ Task 7 (Databricks streaming)      │
    └─ Task 8 (SQL Server/Synapse)       ─┘

kyomi repo:
  Task 3 → Task 10 (ConnectProvider streaming)
            → Task 11 (HTTP endpoint)
              → Task 12 (WASM fetch helpers)
                ├─ Task 13 (chartml provider)  ─┐
                └─ Task 14 (SQL editor)         ├─ Task 15 (delete old)
                                                │    → Task 16 (cleanup + deps)
  Task 9 ───────────────────────────────────────┘
  Task 11 → Task 17 (nginx)

chartml repo:
  Task 18 (Time64) — independent

Parallel groups:
  - Tasks 4, 5, 6, 7, 8 can run in parallel (different provider files)
  - Tasks 13, 14 can run in parallel (different consumer files)
  - Task 18 can run in parallel with everything
```

## Verification

1. SQL editor: query with timestamps → ISO format (not epoch)
2. SQL editor: pagination works (next/prev page, job_id reuse for BigQuery)
3. SQL editor: expired BigQuery job_id auto-retries (no error shown to user)
4. SQL editor: SQL syntax error shows provider error message (not "500")
5. SQL editor: tab restore from localStorage shows "Results expired — click to re-run"
6. chartml: dashboard chart renders correctly
7. chartml: query with >10,000 rows streams to DataFusion (no 10k cap)
8. chartml: large dataset → streamed batches → DataFusion aggregate → small
   chart (neither server nor browser buffers the full dataset)
9. chartml: mid-stream connection drop shows "Query interrupted" (not hang)
10. No `serde_json::Value` in any query result data path
11. No base64 encoding of Arrow data
12. `record_batch_to_json_rows` does not exist
13. No BigQuery-specific code outside the BigQuery provider
14. One HTTP endpoint serves both chartml and SQL editor
15. All 9 providers have native `execute_query_stream_arrow` (no buffered fallback)
16. ConnectProvider streams Arrow batches from WebSocket (no collect_arrow_stream)
17. SQL editor results table renders from DataTable (same as chartml-chart-table)
18. Response Content-Type is `application/vnd.apache.arrow.stream`
19. nginx compresses Arrow responses (check Content-Encoding header)
20. Time64 columns display as HH:MM:SS in both chartml tables and SQL editor
21. Negative limit/offset returns 400 (not silent overflow to large u32)
