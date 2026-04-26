# KYO-169: Local-first cache — Completion Report

## What was built

| Phase | Files created | Files modified | Lines added |
|-------|--------------|----------------|-------------|
| 1. Sync types + migrations | 3 | 2 | ~135 |
| 2. Mutation instrumentation | 0 | 6 | ~650 |
| 3. WebSocket sync protocol | 0 | 6 | ~450 |
| 4. IndexedDB cache | 2 | 2 | ~385 |
| 5. SyncStore + hydration | 1 | 2 | ~350 |
| 6. Sync engine | 1 | 2 | ~425 |
| 7. Page integration | 0 | 5 | ~340 |

**Total: 7 new files, 25 modified files, ~2,735 lines of implementation**

## Review summary

- Tasks reviewed: 10
- Total issues found: 16 (0 critical, 9 major, 7 minor)
- Issues fixed: all 16
- Fix cycles: 7 (one per task that had issues)

## Deferred work

### 1. Tier 2 on-demand detail loading (Task 7.6)
- **What**: Cache detail content (dashboard content, chat messages) in IndexedDB for offline access
- **Why deferred**: Lower priority — list page instant navigation is the main win
- **Impact**: Detail pages still fetch from server on each visit
- **Resolution**: Follow-up ticket

### 2. WORKSPACE_SETTINGS in SyncStore
- **What**: The sync engine receives workspace_settings via bootstrap but doesn't hydrate them into a SyncStore signal
- **Why deferred**: No list page consumes workspace settings from the store yet
- **Impact**: No visible impact — workspace settings are fetched via existing server functions
- **Resolution**: Add when a consumer page needs it

### 3. Sync log pruning
- **What**: `prune_old_entries` exists but no periodic caller
- **Why deferred**: Not needed until the sync_log table grows large (weeks/months of operation)
- **Impact**: sync_log table grows unbounded until pruning is configured
- **Resolution**: Add a background task or startup hook that calls prune_old_entries

## Behavioral divergences from reference

### IndexedDB instead of SQLite-in-WASM
- **What changed**: Used `indexed_db_futures` instead of `rusqlite + sqlite-wasm-vfs`
- **Why**: `rusqlite`'s `links = "sqlite3"` key conflicts with `sqlx`'s sqlite feature at the Cargo workspace resolver level
- **Original behavior**: Plan called for rusqlite with RelaxedIdbVFS
- **User impact**: None — IndexedDB provides the same persistent key-value semantics

## Security notes

- No sensitive data stored in IndexedDB (only list-level metadata: titles, timestamps, IDs)
- Sync log stores entity snapshots in server DB — no PII beyond what's already in the entities table
- WebSocket sync messages are scoped to authenticated workspace members

## Integration notes

- Migration `20260426000000_create_sync_log.sql` will run automatically on server startup
- IndexedDB database `kyomi-sync` is created automatically on first client visit
- No environment/config changes required
- The `indexed_db_futures` crate is added as a wasm32-only dependency

## Compilation status

- `cargo check -p kyomi-ui` (native): clean
- `cargo check -p kyomi-auth`: clean  
- `cargo check -p kyomi-server`: clean (requires `crates/kyomi-ui/dist/` to exist)
- `cargo clippy` on all modified crates: clean
- WASM target: not verified in this session (requires `trunk build`)

## PR

https://github.com/kyomi-ai/kyomi/pull/275
