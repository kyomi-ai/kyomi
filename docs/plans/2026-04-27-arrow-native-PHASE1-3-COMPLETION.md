# Completion Report: Arrow-Native Data Pipeline — Full Implementation

## What was built

| Phase | Repo | Description |
|-------|------|-------------|
| 1A | kyomi-connect | `record_batch` field on QueryResult + sqlx providers (Postgres, MySQL, Redshift) |
| 1B | kyomi-connect | tiberius providers (SQL Server, Synapse) with index-split for ROW_NUMBER |
| 1C | kyomi-connect | REST providers (BigQuery, ClickHouse, Snowflake, Databricks) |
| 2 | kyomi-connect | Arrow wire protocol (QueryFormat, ArrowHeader/Batch/Complete, executor buffered) |
| 3 | kyomi | Server-side consumption (ConnectProvider requests Arrow, registry fix) |
| 4 | — | Already done — direct providers share kyomi-connect code via path dep |
| 5 | kyomi-connect | Remove JSON row population from all 9 providers (rows: None, Arrow only) |
| — | kyomi | SQL editor migrated to `record_batch_to_json_rows`, `query_result_to_arrow_ipc` deleted |
| — | kyomi-connect | Arrow streaming for large queries (execute_query_stream_arrow on trait) |
| — | chartml | Table renderer fix: prefer `get_string` for temporal columns (v5.0.2 released) |
| — | kyomi-connect | ClickHouse DateTime null fix (RFC3339 fallback in json_value_to_arrow) |
| — | kyomi-connect | 750 lines of unit tests for all JSON-to-Arrow provider conversions |

## Branches and PRs

- **kyomi-connect**: `feat/arrow-native-data-pipeline` — PR #3
- **kyomi**: `feat/arrow-native-server-consumption` — PR #303
- **chartml**: v5.0.2 tagged and released on main

## Linear tickets closed

- **KYO-231**: Remove JSON row path from providers — Done
- **KYO-232**: Arrow streaming for large queries — Done
- **KYO-238**: SQL editor migration to RecordBatch — Done
- **KYO-239**: chartml table renderer fix — Done

## E2E test results (13/13 passed)

Test script: `scripts/e2e-regression/test-arrow-pipeline.cjs`

| Test | Result |
|------|--------|
| DateTime values (toDateTime) | ISO timestamps, not epoch |
| Date values (toDate) | YYYY-MM-DD |
| Number values | Correct |
| String values | Correct |
| NULL values | null displayed, nullable timestamps work |
| Mixed types in one query | All correct, no epoch millis |
| Real table with non-null timestamps | ISO format |
| Pagination with Arrow data | Works, no epoch millis |

Screenshots saved to `/tmp/e2e-arrow/` confirming visual correctness.

## Remaining work

### Path dependencies need reverting before release

`Cargo.toml` in kyomi repo uses path dependencies pointing at `../kyomi-connect/crates/...`.
Must publish new versions of `kyomi-connect-protocol` and `kyomi-datasource` to crates.io,
then revert to versioned dependencies.

### `CONNECT_URL` missing from k8s ConfigMap

Unrelated to Arrow pipeline — noted during Connect debugging at session start. The server
defaults to `wss://localhost:8003/connect/v1` which is wrong for prod.

### JSON streaming functions retained

`_row_value_to_json` functions are kept in providers because the JSON streaming path
(`execute_query_stream` returning `QueryStreamEvent`) still exists for `format == Json`
backward compatibility. These can be removed once the JSON streaming path is deleted entirely.

## Compilation status

- **kyomi-connect**: `cargo check` clean (1 warning: `tds_row_to_arrow` unused — reserved for future tiberius streaming override)
- **kyomi**: `cargo check -p kyomi-server` clean, pre-commit hooks pass (host + wasm32 clippy)
- **chartml**: all tests pass, v5.0.2 published to crates.io
