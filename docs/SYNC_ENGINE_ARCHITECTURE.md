# Sync Engine Architecture

## Overview

Kyomi uses a Linear-style local-first sync engine to make list page navigations
instant on return visits. Metadata is cached in IndexedDB and kept current via
a WebSocket-based sync protocol.

This document is the **authoritative reference** for how the sync engine works.
If the code doesn't match this document, the code is wrong.

## Data Flow

### First visit (empty IndexedDB)

```
1. App loads → open IndexedDB "kyomi-sync"
2. Check _meta store: no schemaHash or lastSyncId → FULL BOOTSTRAP required
3. Send sync_bootstrap over WebSocket
4. Server streams all Tier 1 metadata as sync_action messages
5. Client writes each entity to IDB + upserts into SyncStore (reactive signals)
6. Server sends sync_complete { last_sync_id: N }
7. Client stores lastSyncId=N and schemaHash=CURRENT in IDB _meta
8. mark_initialized() → pages render from SyncStore signals
```

### Return visit (valid IndexedDB)

```
1. App loads → open IndexedDB "kyomi-sync"
2. Check _meta store: schemaHash matches current → LOCAL BOOTSTRAP
3. Read all entities from IDB → bulk-set into SyncStore signals
4. mark_initialized() → pages render INSTANTLY from cached data
5. WebSocket connects → send sync_delta { last_sync_id: N }
6. Server streams only changes since N as sync_action messages
7. Client upserts each change into IDB + SyncStore
8. sync_complete → update lastSyncId in _meta
```

### Return visit (schema mismatch)

```
1. App loads → open IndexedDB "kyomi-sync"
2. Check _meta store: schemaHash ≠ current → STALE, treat as first visit
3. Wipe all IDB stores (entity_cache, sync_cursors)
4. Proceed with FULL BOOTSTRAP (same as first visit)
```

### WebSocket reconnect (same tab, network blip)

```
1. WebSocket reconnects → send sync_delta { last_sync_id: N }
   (N is the in-memory cursor, always current)
2. Server streams changes since N
3. Client upserts into IDB + SyncStore
4. sync_complete → update cursor
```

### sync_reset (server says cursor too old / pruned)

```
1. Server sends sync_reset
2. Client wipes all IDB entity data + resets cursor to 0
3. Client sends sync_bootstrap (full re-bootstrap)
```

## Schema Hash

The schema hash is a constant string embedded in the WASM binary that changes
whenever the sync data format changes. Examples of changes that require a new
schema hash:

- Fields added/removed from DashboardListItem, ChatSessionItem, WatchListItem
- Server-side sync query changes (different columns, different JSON shape)
- Entity types added/removed from the bootstrap

The schema hash is stored in IDB alongside the lastSyncId. On page load, the
client compares the stored hash with the compiled-in hash. A mismatch means
the cached data was written by an older version of the code and may not
deserialize correctly — triggering a full wipe + re-bootstrap.

**Location:** `crates/kyomi-ui/src/cache/db.rs` → `SCHEMA_HASH` constant.

**When to bump:** Any PR that changes the shape of data flowing through the
sync protocol. Include a comment in the PR noting the schema hash change.

## IndexedDB Schema

Database name: `kyomi-sync`

### Object stores

| Store | Key type | Purpose |
|-------|----------|---------|
| `entity_cache` | Out-of-line string: `"{entity_type}\0{workspace_id}\0{entity_id}"` | Cached entity metadata (JSON blobs) |
| `sync_cursors` | Out-of-line string: workspace_id | `lastSyncId` per workspace |
| `_meta` | Out-of-line string: key name | `schemaHash` — the compiled-in hash at time of last successful bootstrap |

### entity_cache value format

```json
{
  "entity_id": "abc-123",
  "data": "{\"dashboard_id\":\"abc-123\",\"title\":\"...\", ...}",
  "updated_at": "2026-04-26T12:00:00Z"
}
```

The `data` field is a JSON string (double-encoded) matching the shape of the
corresponding Rust struct (`DashboardListItem`, `ChatSessionItem`, etc.).

## Entity Types

| Constant | Bootstrap source | Client struct | Notes |
|----------|-----------------|---------------|-------|
| `dashboard` | `list_dashboards_for_sync` | `DashboardListItem` | `doc_type = 'dashboard'` |
| `knowledge` | `list_knowledge_for_sync` | `DashboardListItem` | `doc_type = 'knowledge'` |
| `chat_session` | `list_sessions_for_sync` | `ChatSessionItem` | `session_type = 'chat'` only |
| `watch` | `list_watches_for_sync` | `WatchListItem` | All watches |
| `workspace_settings` | `get_workspace_settings_for_sync` | `serde_json::Value` | Singleton per workspace |

### Filtering rules

- **chat_session**: Only `session_type = 'chat'` sessions are synced. Watch
  execution logs (`session_type = 'watch'`) and copilot sessions
  (`session_type = 'copilot'`) are excluded from both bootstrap and live sync.

- **dashboard/knowledge**: Filtered by `doc_type` column. Both use the same
  `dashboards` table but are separate entity types in the sync protocol.

## SyncStore (Reactive In-Memory Store)

`SyncStore` is a `Copy`-able handle backed by `StoredValue<SendWrapper<...>>`.
It holds `ArcRwSignal<Vec<T>>` for each entity type. Pages read from these
signals — they never query IDB or the server directly for list data.

**Provided at:** Layout level via `provide_context(sync_store)`

**Lifecycle:**
1. Created empty in Layout
2. Hydrated from IDB (local bootstrap) — signals populated, pages render
3. Updated by sync engine (bootstrap/delta responses) — signals upserted
4. `initialized()` signal gates page rendering (skeleton until true)

**Key rule:** The SyncStore is the read source. IDB is persistence only. The
server is the source of truth.

## Sync Engine

`start_sync_engine()` is called once from `SyncEngineStarter` inside the
`WebSocketProvider` scope.

### Subscriptions

| WS message type | Handler |
|-----------------|---------|
| `sync_action` | `apply_sync_action` — upserts entity into SyncStore + writes to IDB |
| `sync_complete` | Updates in-memory cursor + persists to IDB + marks store initialized |
| `sync_reset` | Wipes IDB entities + resets cursor + sends sync_bootstrap |

### Connection state handling

The engine watches `WebSocketContext::connection_state`. On each transition to
`Connected`:

- **First connect** (in-memory cursor = 0): send `sync_bootstrap`
- **Reconnect** (in-memory cursor > 0): send `sync_delta { last_sync_id: N }`

The in-memory cursor is set by `sync_complete` and lives for the tab's
lifetime. It is NOT read from IDB on reconnect — IDB cursor is only for
cross-session persistence (determining local bootstrap vs full bootstrap on
page load).

## Server-Side Components

### sync_log table

Append-only log of all metadata mutations. Each row has a monotonic `sync_id`
(BIGSERIAL on Postgres, AUTOINCREMENT on SQLite).

**Pruned** by a background task every 24 hours (default: 30-day retention).
Clients with a cursor older than the oldest remaining entry receive `sync_reset`.

### WebSocket handlers

| Client message | Server handler |
|----------------|---------------|
| `sync_bootstrap` | Queries all list functions, streams entities as sync_actions, sends sync_complete |
| `sync_delta` | Queries sync_log since last_sync_id, streams entries, sends sync_complete or sync_reset |

### Mutation instrumentation

Every service function that mutates metadata (create/update/delete on
dashboards, chat sessions, watches, workspace settings) writes a sync_log
entry AND broadcasts a sync_action over WebSocket to connected workspace
members.

## Common Failure Modes and Recovery

| Failure | Symptom | Recovery |
|---------|---------|----------|
| Broken bootstrap (server bug) | IDB has cursor but no/wrong data | Schema hash bump in next deploy forces re-bootstrap |
| Schema change without hash bump | Deserialization failures, empty lists | Bump SCHEMA_HASH, deploy |
| Cursor older than sync_log retention | Delta returns nothing useful | Server sends sync_reset → auto re-bootstrap |
| IDB corrupted (multi-tab race) | Various errors | onupgradeneeded wipes stores; or user clears site data |
| WebSocket disconnected | Stale data shown | Reconnect → delta catch-up |

## Files

| File | Purpose |
|------|---------|
| `crates/kyomi-types/src/sync.rs` | SyncAction, SyncResponse, entity_types constants |
| `crates/kyomi-auth/src/sync_log_service.rs` | Server-side sync_log CRUD |
| `crates/kyomi-auth/src/dashboard_service.rs` | `list_dashboards_for_sync`, `list_knowledge_for_sync` |
| `crates/kyomi-auth/src/chat_service.rs` | `list_sessions_for_sync` |
| `crates/kyomi-auth/src/watch_service.rs` | `list_watches_for_sync` |
| `crates/kyomi-auth/src/workspace_service.rs` | `get_workspace_settings_for_sync` |
| `apps/server/src/routes/websocket.rs` | `handle_sync_bootstrap`, `handle_sync_delta` |
| `crates/kyomi-ui/src/cache/db.rs` | IndexedDB operations, SCHEMA_HASH |
| `crates/kyomi-ui/src/cache/store.rs` | SyncStore (reactive signals) |
| `crates/kyomi-ui/src/cache/sync_engine.rs` | Client-side sync engine |
| `crates/kyomi-ui/src/components/layout.rs` | SyncStore provider, hydration Effect, SyncEngineStarter |
