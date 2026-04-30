# KYO-243 Completion Report: Unified Arrow Query Path

## What was built

One HTTP endpoint (`POST /api/v1/query-arrow`) replaces three separate query data paths. Arrow IPC binary transport — no base64, no JSON conversion. All 9 providers have native `execute_query_stream_arrow` implementations. Both chartml (streaming) and SQL editor (paginated) use the same endpoint.

### Commits (kyomi repo)

| Commit | Description |
|--------|-------------|
| e395b30a | ConnectProvider streaming override + multi-batch concat |
| 3e80f93b | POST /api/v1/query-arrow — unified Arrow IPC HTTP endpoint |
| be4d3196 | WASM fetch helpers for Arrow IPC endpoint |
| ff6e41c1 | Migrate chartml provider from base64 server fn to Arrow fetch |
| 19b5c85d | Migrate SQL editor from server fn to Arrow fetch endpoint |
| 3170ca27 | Delete fetch_query_page and dead query count cache |
| 860e89c4 | Document kyomi-connect 1.2.0 publish in patch block |

### Commits (kyomi-connect repo, v1.2.0)

| Commit | Description |
|--------|-------------|
| ef1946c | Add job_id parameter to execute_query trait and QueryResult |
| 92412a0 | BigQuery job_id pass-through + wire protocol job_id field |
| 303a250 | Add execute_query_stream_arrow overrides for all 9 providers |
| cd5636b | Bump all crates to 1.2.0 |

### Commits (chartml repo)

| Commit | Description |
|--------|-------------|
| 5da766f | Add Time64/Time32 → HH:MM:SS formatting in arrow_to_string |

## Review summary

- Tasks reviewed: 13
- Total issues found: 22 (across all review cycles)
- Issues fixed: 22
- Fix cycles: ~15 review→fix→re-review rounds

## Deferred work

| Item | Linear | Why deferred |
|------|--------|-------------|
| chart_builder migration to Arrow fetch | KYO-258 | chart_builder.rs is a separate consumer; migrating it would expand scope |
| Provider creation DRY extraction | KYO-259 | Out of scope — architectural refactor, not blocking functionality |
| kyomi-connect QueryResult.rows removal | — | Breaking change requiring 1.3.0 publish; rows field is unused but harmless |
| Streaming metadata delivery | — | Arrow IPC format limitation — per-batch schema metadata not transmitted |
| True incremental streaming (ReadableStream) | KYO-244 | Requires DataFusion streaming input — separate effort |

## Behavioral divergences from reference

| What changed | Why | Original behavior |
|---|---|---|
| SQL editor results render from DataTable instead of JSON rows | Arrow columnar format is the native transport | JSON rows via serde_json::Value |
| Tab restore shows "Results expired — click to re-run" | DataTable is #[serde(skip)] — cannot be serialized to localStorage | Auto-re-executed on restore |
| Null display uses uppercase "NULL" consistently | Aligned Arrow and JSON paths | Mixed case in some paths |

## Security notes

- Auth gating: `AuthUser` extractor on the new endpoint — same session cookie auth as Leptos server fns
- Datasource resolution: NotFound → 403 (prevents slug enumeration), DB errors → 500
- Query execution timeout: `DATASOURCE_TIMEOUT_QUERY` on both paginated and streaming paths
- Provider close: called in all paths to prevent connection pool leaks

## Integration notes

- **chartml repo**: Branch `jason/kyo-243-time64-formatting` needs PR + merge + chartml publish
- **nginx**: `application/vnd.apache.arrow.stream` added to gzip_types on NAS (192.168.1.100)
- **kyomi-connect**: v1.2.0 published to crates.io; kyomi uses path patches for local dev
- **PR**: https://github.com/kyomi-ai/kyomi/pull/311

## Compilation status

- `cargo check --workspace`: clean
- `cargo clippy -p kyomi-server -- -D warnings`: clean
- `cargo clippy -p kyomi-ui -- -D warnings`: clean (host)
- WASM clippy: clean
