# ChartML DataFusion Streaming Batches — Implementation Plan

## Problem

The `TransformMiddleware::transform` method accepts `&IndexMap<String, DataTable>`
where each `DataTable` wraps a single `RecordBatch`. This means the entire
dataset must be fetched and held as one contiguous RecordBatch before the
transform can run. For large datasets (millions of rows), this requires the
caller to buffer everything upfront.

The unified Arrow query path (separate plan, kyomi repo) streams RecordBatches
incrementally from the server to the browser. The transform middleware
needs to accept these batches as they arrive rather than requiring all
data upfront.

Additionally, `DataFusionTransform` uses `SessionContext::new()` with
defaults — `UnboundedMemoryPool`, no `DiskManager`. On native (server-side),
DataFusion supports spill-to-disk for sorts and aggregations, but this is
never triggered because no memory limit is configured.

## Goal

1. TransformMiddleware accepts streamed RecordBatches — the caller feeds
   batches one at a time as they arrive from the HTTP stream
2. DataFusion registers batches into MemTable incrementally — no concat
   into one giant RecordBatch
3. Existing two-tier cache (MemoryBackend + IndexedDbBackend) stores the
   fetched data so page refreshes don't re-query the database
4. Native SessionContext configured with memory limits and DiskManager so
   server-side transforms can spill to disk for large datasets
5. WASM stays in-memory for now — OPFS spill is a future enhancement

## Current Architecture

```
Provider.fetch() → DataTable (single RecordBatch)
    ↓
Resolver caches DataTable to tier-1 (Memory) + tier-2 (IndexedDB)
    ↓
TransformMiddleware.transform(sources: &IndexMap<String, DataTable>)
    ↓
DataFusionTransform:
    SessionContext::new()  ← no memory limits, no disk manager
    MemTable::try_new(schema, vec![vec![batch]])  ← one batch per source
    Run SQL → Aggregate → Forecast pipeline
    ↓
Small aggregated DataTable → chart renderer
```

## Target Architecture

```
Provider.fetch_batches() → (SchemaRef, Vec<RecordBatch>)
    ↓
Resolver caches concat'd DataTable to tier-1 + tier-2
    ↓
TransformMiddleware.transform_batches(
    sources: IndexMap<String, (SchemaRef, Vec<RecordBatch>)>
)
    ↓
DataFusionTransform:
    SessionContext with FairSpillPool + DiskManager (native)
    SessionContext with bounded MemoryPool (WASM, no spill)
    MemTable::try_new(schema, vec![batches])  ← multiple batches, no concat
    Run SQL → Aggregate → Forecast pipeline
    ↓
Small aggregated DataTable → chart renderer

Cache hit path (page refresh):
    tier-1 or tier-2 → DataTable → transform() (existing API, one batch)
    Same result — aggregation is identical regardless of batch count.
```

---

## Tasks

All tasks are in the chartml repo. Ordered by dependency.

### Task 1: DataTable helper methods

Add `into_record_batch()` and `schema() -> SchemaRef` methods to DataTable.
These are needed by the resolver to extract the batch for caching and by
`transform_batches`'s default implementation.

DataTable itself stays single-batch — multi-batch complexity is only
needed by the transform, not by renderers. `get_string(row, col)`,
`get_f64(row, col)`, `num_rows()` all work cleanly on a single
RecordBatch with direct array indexing.

**Files:**
- `crates/chartml-core/src/data.rs`

**Depends on:** nothing

### Task 2: TransformMiddleware — add transform_batches

Add `transform_batches` method to the `TransformMiddleware` trait with a
default implementation that concatenates batches into DataTables and
delegates to `transform()`. This preserves backward compatibility —
existing middleware implementations and all tests work unchanged.

```rust
async fn transform_batches(
    &self,
    sources: &IndexMap<String, (SchemaRef, Vec<RecordBatch>)>,
    spec: &TransformSpec,
    context: &TransformContext,
) -> Result<TransformResult, ChartError> {
    // Default: concat → DataTable → delegate to transform()
}
```

**Files:**
- `crates/chartml-core/src/plugin/transform.rs`

**Depends on:** Task 1

### Task 3: DataSourceProvider — add fetch_batches

Add `fetch_batches` method to the `DataSourceProvider` trait (or whatever
the provider trait is called) with a default implementation that delegates
to `fetch()` and wraps the single batch.

```rust
async fn fetch_batches(
    &self,
    request: FetchRequest,
) -> Result<FetchBatchResult, FetchError> {
    // Default: fetch() → wrap single batch
}
```

The kyomi-side consumer (`chartml_provider.rs`) overrides `fetch_batches`
to use the HTTP streaming endpoint — that change is covered by the unified
Arrow query path plan (kyomi repo). Other providers (inline, mock) use
the default delegation.

**Files:**
- `crates/chartml-core/src/plugin/data_source.rs`

**Depends on:** nothing (parallel with Task 2)

### Task 4: DataFusionTransform — override transform_batches

Override `transform_batches` in `DataFusionTransform` to register all
batches directly into MemTable without concatenation.

`MemTable::try_new(schema, vec![batches])` already accepts
`Vec<Vec<RecordBatch>>`. Pass one partition with all batches. DataFusion's
query engine iterates over them naturally during execution.

Include the single-source alias logic (same as existing `transform`).

**Files:**
- `crates/chartml-datafusion/src/lib.rs`

**Depends on:** Task 2

### Task 5: SessionContext configuration

Replace `SessionContext::new()` with a configured context:

- **Native**: `FairSpillPool` with memory limit + `DiskManager` (OS temp
  directory). Enables spill-to-disk for sorts and aggregations on large
  datasets.
- **WASM**: `SessionConfig` with batch size tuning, default memory
  management. No spill (no disk access). OPFS spill is a future
  enhancement.

```rust
fn build_session_context() -> SessionContext {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let runtime = RuntimeEnvBuilder::new()
            .with_memory_limit(512 * 1024 * 1024, 0.8)
            .with_disk_manager_os()
            .build_arc()
            .unwrap();
        SessionContext::new_with_config_rt(SessionConfig::new(), runtime)
    }

    #[cfg(target_arch = "wasm32")]
    {
        SessionContext::new_with_config(SessionConfig::new())
    }
}
```

Both `transform()` and `transform_batches()` use `build_session_context()`.

**Files:**
- `crates/chartml-datafusion/src/lib.rs`

**Depends on:** nothing (parallel with Tasks 2-4, but commit after Task 4
since both modify the same file)

### Task 6: Resolver — call fetch_batches + cache integration

Update the resolver's fetch path to call `fetch_batches` when available.
Cache the concatenated DataTable (same format, same IndexedDB codec).
Pass the original batches to `transform_batches`.

**Cache miss path:**
```
fetch_batches() → (schema, Vec<RecordBatch>)
→ concat → DataTable → cache to tier-1 + tier-2
→ original (schema, batches) → transform_batches()
```

**Cache hit path (page refresh):**
```
tier-1 or tier-2 → DataTable → transform() (existing API)
```

Both paths produce identical results. The cache hit path uses one
batch (the cached DataTable), which is fine — the aggregation runs
the same query regardless of how many batches the input is split into.

No cache format changes. No IndexedDB codec changes. The existing
two-tier cache stores DataTables exactly as it does today.

**Files:**
- `crates/chartml-core/src/resolver/mod.rs`

**Depends on:** Tasks 2, 3 (trait methods exist)

---

## Task dependency graph

```
Task 1 (DataTable helpers) → Task 2 (transform_batches trait)
                               → Task 4 (DataFusion override) → Task 5 (SessionContext)
Task 3 (fetch_batches trait) ─┐
Task 2 ───────────────────────┴─ Task 6 (resolver integration)

Parallel groups:
  - Tasks 1 and 3 can run in parallel
  - Task 5 can run in parallel with Task 6 (commit after Task 4)
```

## Verification

1. All existing chartml tests pass unchanged (backward compat via defaults)
2. DataFusion transform produces identical results with 1 batch vs N batches
3. Cache hit on page refresh returns data without re-querying
4. Native: DataFusion spills to disk when dataset exceeds memory limit
5. WASM: Transform works in-memory for datasets that fit in browser memory
6. Large dataset (>10k rows) flows through without the 10k cap regression

## Future enhancements (not in scope)

- **OPFS spill-to-disk in WASM**: Implement ObjectStore over OPFS, run
  DataFusion in a Web Worker, configure DiskManager with OPFS backend.
  Enables browser-side processing of datasets larger than available memory.
- **Multi-part cache blobs**: Store batches separately in IndexedDB to
  avoid concat-then-split round-trip on cache miss path.
- **Streaming cache writes**: Write batches to IndexedDB as they arrive
  rather than waiting for all batches to complete.
