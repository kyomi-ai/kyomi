# Arrow-Native Data Pipeline — Implementation Plan

## Problem

Every datasource provider serializes native typed query results into
`Vec<Vec<serde_json::Value>>` then the server re-parses those JSON values back
into Arrow RecordBatch. This loses type fidelity — timestamps become epoch
millis, decimals lose precision, booleans become 0/1. The whole point of the
Rust rewrite was native types end to end.

## Current State

Branch `feat/arrow-native-data-pipeline` in `kyomi-connect` (rebased onto main
2026-04-27) has the infrastructure:

- **`ArrowResultBuilder`** (946 lines) — typed column builders with `append_*`
  methods for every SimpleType
- **Per-provider `_row_to_arrow` functions** — all 9 providers (Postgres, MySQL,
  Redshift, ClickHouse, SQL Server, Synapse, BigQuery, Snowflake, Databricks)
- **`ArrowStreamEvent`** — new streaming protocol (Schema → Batch → Complete)
  with base64-encoded IPC bytes
- **`QueryFormat`** enum — opt-in Arrow vs JSON
- **`drive_sqlx_stream_arrow`** — shared sqlx streaming helper for Arrow
- **`make_arrow_stream_channel`** — channel factory for Arrow streams

None of this is wired into `execute_query` yet. The `_row_to_arrow` functions
exist but are never called.

A stash exists with 654 lines of old JSON conversion function removals — the
planned next step that was never applied.

## Provider Driver Map

| Provider | Driver | Connection | Arrow Difficulty |
|----------|--------|-----------|-----------------|
| PostgreSQL | sqlx | Native TCP | Easy — typed rows |
| MySQL | sqlx | Native TCP | Easy — typed rows |
| Redshift | sqlx | Native TCP (PG wire) | Easy — reuses Postgres |
| SQL Server | tiberius | Native TDS | Easy — typed rows |
| Synapse | tiberius | Native TDS | Easy — same as SQL Server |
| BigQuery | reqwest | REST/JSON | Medium — parse JSON once into Arrow |
| ClickHouse | reqwest | HTTP/JSON | Medium — parse JSON once into Arrow |
| Snowflake | reqwest | REST/JSON | Medium — parse JSON once into Arrow |
| Databricks | reqwest | REST/JSON | Medium — parse JSON once into Arrow |

## Repositories Involved

- **kyomi-connect** (`~/repos/kyomi-connect`) — agent-side providers, protocol
- **kyomi** (`~/repos/kyomi`) — server-side Connect consumer, server functions, UI

## Implementation Phases

### Phase 1: Wire Arrow into execute_query (kyomi-connect)

**Goal:** Each provider's `execute_query` populates a `RecordBatch` on
`QueryResult` instead of (or alongside) JSON rows.

**Approach:** Add `record_batch: Option<RecordBatch>` field to `QueryResult`.
Each provider calls its existing `_row_to_arrow` function to build the batch.
Keep JSON rows populated too for backward compat during transition.

**Files:**
- `crates/kyomi-connect-protocol/src/wire.rs` — add `record_batch` field to
  `QueryResult` (skip serde, it's not serializable as JSON)
- `crates/kyomi-datasource/src/provider.rs` — update `QueryResult` struct
- `crates/kyomi-datasource/src/providers/postgres.rs` — wire `pg_row_to_arrow`
  into `execute_query`
- `crates/kyomi-datasource/src/providers/mysql.rs` — wire `mysql_row_to_arrow`
- `crates/kyomi-datasource/src/providers/redshift.rs` — wire
  `redshift_row_to_arrow`
- `crates/kyomi-datasource/src/providers/sqlserver.rs` — wire `tds_row_to_arrow`
- `crates/kyomi-datasource/src/providers/synapse.rs` — wire `tds_row_to_arrow`
- `crates/kyomi-datasource/src/providers/bigquery.rs` — wire
  `bigquery_row_to_arrow`
- `crates/kyomi-datasource/src/providers/clickhouse.rs` — wire
  `clickhouse_row_to_arrow`
- `crates/kyomi-datasource/src/providers/snowflake.rs` — wire
  `snowflake_row_to_arrow`
- `crates/kyomi-datasource/src/providers/databricks.rs` — wire
  `databricks_row_to_arrow`

**Tests:** Each provider's existing tests must still pass. Add tests that
verify `record_batch` is populated with correct types (timestamp columns are
Timestamp, not String).

**Deliverable:** `cargo test` passes, every provider populates `record_batch`.

---

### Phase 2: Arrow over the Connect wire protocol (kyomi-connect)

**Goal:** When the server requests Arrow format, the Connect agent sends Arrow
IPC bytes instead of JSON rows.

**Approach:** The `ArrowStreamEvent` and `QueryFormat` types already exist on
the branch. Wire them:

1. Add `format: QueryFormat` field to `ConnectRequest` (defaults to `Json`)
2. When `format == Arrow`, the executor calls the Arrow streaming path
   (`drive_sqlx_stream_arrow` for sqlx providers, equivalent for REST/tiberius)
3. Serialize `ArrowStreamEvent` responses instead of `QueryStreamEvent`
4. For buffered (non-streaming) queries: serialize the `RecordBatch` as IPC
   bytes in a single `ArrowStreamEvent::Batch`

**Files:**
- `crates/kyomi-connect-protocol/src/wire.rs` — add `format` to request
- `crates/kyomi-connect/src/executor.rs` — branch on format, call Arrow path
- `crates/kyomi-datasource/src/providers/postgres.rs` — implement
  `execute_query_stream_arrow` using `drive_sqlx_stream_arrow`
- Same for mysql, redshift (sqlx providers)
- `crates/kyomi-datasource/src/providers/sqlserver.rs` and synapse — Arrow
  streaming for tiberius
- REST providers (bigquery, clickhouse, snowflake, databricks) — Arrow
  streaming from JSON responses

**Tests:** Integration test: send Arrow-format request, verify response
contains valid IPC bytes that decode to a RecordBatch with correct schema.

**Deliverable:** Connect agent can respond in Arrow format when requested.

---

### Phase 3: Server-side Arrow consumption (kyomi)

**Goal:** The Kyomi server requests Arrow format from Connect and passes
RecordBatch through to chartml without re-parsing.

**Files:**
- `crates/kyomi-datasource/src/connect/provider.rs` — request
  `QueryFormat::Arrow`, deserialize `ArrowStreamEvent` responses, reconstruct
  `RecordBatch` from IPC bytes
- `crates/kyomi-ui/src/server_fns/datasources.rs`:
  - `query_datasource_arrow()` — use `record_batch` directly instead of calling
    `query_result_to_arrow_ipc()`
  - Delete `query_result_to_arrow_ipc()` (the 150-line JSON re-parser)
- `crates/kyomi-ui/src/server_fns/sql_editor.rs`:
  - `execute_sql_query_provider()` — for Connect datasources, get Arrow result;
    for the SQL editor UI response type, convert RecordBatch → rows (the SQL
    editor table needs JSON rows for its own rendering, but this is a display
    concern, not a data pipeline concern)

**Tests:** End-to-end: query via Connect datasource, verify chartml DataTable
receives a RecordBatch with Timestamp columns (not String columns containing
epoch millis).

**Deliverable:** `query_result_to_arrow_ipc` is deleted. Timestamps display
correctly in chartml tables.

---

### Phase 4: Direct providers on server (kyomi)

**Goal:** Server-side direct providers (non-Connect — used when datasources
are configured without Connect) also return RecordBatch.

**Context:** The server has its own copy of the provider implementations for
direct connections. These also go through the JSON roundtrip via
`query_result_to_arrow_ipc`. With the kyomi-connect providers now returning
RecordBatch, the server-side `create_query_provider` path for direct providers
should do the same.

**Files:**
- Depends on how server-side direct providers are structured — they may share
  the kyomi-connect provider code or have their own. Investigate and align.

**Deliverable:** Both Connect and direct provider paths produce RecordBatch.

---

### Phase 5: Remove JSON row path (kyomi-connect)

**Goal:** Remove deprecated JSON conversion functions and the `rows` field
from `QueryResult`.

**Approach:** Apply the stashed deletions (654 lines), remove `rows` field,
update all consumers.

**Files:**
- All 9 provider files — remove `_row_value_to_json` functions
- `crates/kyomi-datasource/src/provider.rs` — remove `rows` from `QueryResult`
- `crates/kyomi-connect-protocol/src/wire.rs` — remove JSON streaming events
  if fully superseded

**Tests:** Full test suite passes with no JSON path remaining.

**Deliverable:** Single code path — Arrow only. No `serde_json::Value` in the
data pipeline.

---

## Verification Criteria

1. Query a Postgres table with `timestamptz` columns via Connect — chartml
   table shows ISO timestamps, not epoch millis
2. Query the same table — SQL editor shows formatted dates
3. Query with `NUMERIC(38,18)` column — no precision loss
4. All existing dashboard charts render identically (no regression)
5. `query_result_to_arrow_ipc` function no longer exists in codebase
6. `serde_json::Value` does not appear in the query result data path
