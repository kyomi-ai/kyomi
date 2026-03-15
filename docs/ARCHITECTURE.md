# Architecture Overview

## Deployment Modes

Kyomi runs in two modes from a single codebase:

### SaaS Mode (app.kyomi.ai)
- **Frontend**: Vite-built React SPA served by nginx from static files
- **Backend**: Rust binary (`kyomi-api`) on port 8002
- **Database**: PostgreSQL
- **Cache/KV**: Redis
- **Reverse proxy**: nginx routes `/api/v1/*` to backend, serves frontend for everything else

### Standalone / Self-Hosted Mode (dev.kyomi.ai)
- **Single binary** serves both frontend and API on port 3000
- **Frontend**: Embedded via `rust-embed` (compiled into binary in release; reads from `apps/frontend/dist/` in debug)
- **Database**: SQLite at `{DATA_DIR}/kyomi.db` (WAL mode, single connection)
- **Cache/KV**: In-memory (no Redis required)
- **Secrets**: Auto-generated on first run, stored in `{DATA_DIR}/config.toml` (0o600 permissions)
- **Env vars**: `SELF_HOSTED=true`, `PORT=3000`, `DATA_DIR=./data/`

---

## Crate Architecture

All Rust code lives under `apps/backend-rust/crates/`:

```
kyomi-api          Binary entry point, HTTP routes (axum), frontend serving
kyomi-agent        LLM agent: tools, prompt building, conversation orchestration
kyomi-auth         Authentication (passkeys, passwords, TOTP, JWT)
kyomi-core         Shared: database pool, config, KV store, constants
kyomi-knowledge    Knowledge files, vector search, graph expansion, chunking
kyomi-embed        ONNX embedding model (BGE-small-en-v1.5, compiled in via build.rs)
kyomi-datasource   Datasource providers (BigQuery, Postgres, MySQL, ClickHouse, etc.)
kyomi-slack        Slack integration
```

### Key embedding patterns
- **Frontend**: `rust-embed` in `kyomi-api/src/frontend.rs`
- **ONNX model**: `include_bytes!()` via `kyomi-embed/build.rs` (downloaded at compile time)
- **ChartML spec**: `include_str!()` in `kyomi-agent/src/prompt.rs`
- **MCP app**: `include_str!()` in `kyomi-api/src/routes/mcp.rs`
- **Constants**: `include_str!()` in `kyomi-core/src/constants.rs` with disk override

---

## Frontend Architecture

React SPA in `apps/frontend/`:

- **UI library**: shadcn/ui + Radix UI primitives + Tailwind CSS
- **State**: React Context (AuthContext, ThemeContext)
- **HTTP**: Axios via `apiClient` from AuthContext
- **Editor**: Monaco (markdown), Tiptap (rich text/charts)
- **DnD**: `@dnd-kit/core` + `@dnd-kit/sortable`
- **Charts**: ChartML custom web components

### Key UI patterns
- **Modals**: `components/Modal.jsx` — portal-based, size variants (sm/md/lg/xl/full)
- **Confirmations**: `components/ConfirmDialog.jsx` + `hooks/useConfirm.js` — async promise-based API, replaces `window.confirm()`
- **Dropdowns**: `components/ui/dropdown-menu.jsx` — Radix-based
- **Inline editing**: `components/InlineEditableTitle.jsx`

---

## Dev Environment

### Two backend modes (they conflict on port 8002)

1. **Container** (`kyomi-api-dev`): Started by `docker-compose.dev.yml`. Runs old compiled binary. Does NOT reflect code changes.
2. **Local binary**: `scripts/dev/start-rust-backend.sh`. Runs `cargo run` with `.env`. Reflects current code.

### Standalone dev testing

dev.kyomi.ai reverse proxy points to `192.168.1.200:3000`. The standalone binary must run on port 3000.

In **debug mode**, `rust-embed` reads frontend from `apps/frontend/dist/` at runtime — no Rust rebuild needed for frontend-only changes. Just rebuild the frontend (`npm run build` in `apps/frontend/`) and refresh.

In **release mode**, frontend is compiled into the binary. Any frontend change requires a full `cargo build --release` (~5 min).

### NAS (192.168.1.100)
- **SSH**: `ssh 192.168.1.100`
- **File transfer**: `cat file | ssh 192.168.1.100 "cat > /tmp/file"` then `sudo cp` (scp broken)
- **Nginx configs**: `/etc/nginx/sites-enabled/`
- **Static frontend**: `/volume1/web/kyomi.ai/app/`
- **SSL**: `/etc/nginx/ssl/` (wildcard cert for *.kyomi.ai)

---

## Knowledge System

### Data flow
```
User/Agent writes markdown
  → knowledge_files table (source of truth)
  → Chunked (~500 tokens, ~100 token overlap)
  → Embedded (BGE-small-en-v1.5)
  → knowledge_chunks table (retrieval index)
  → Table refs extracted → knowledge_file_tables (graph expansion)
```

### Retrieval
- **Path A**: Agent sees file tree in system prompt → reads file directly
- **Path B**: Semantic search → chunks match → full parent documents returned
- **Graph expansion**: table ↔ knowledge file cross-references via single JOINs

### Agent tools
- `ReadKnowledgeFile`, `ListKnowledgeFiles`, `WriteKnowledgeFile`, `EditKnowledgeFile` — file CRUD
- `SearchKnowledge` — unified semantic search across tables, knowledge files

---

## Database

### PostgreSQL (SaaS)
- Migrations: `apps/backend-rust/migrations/`
- pgvector for embeddings

### SQLite (Standalone)
- Migrations: `apps/backend-rust/migrations-sqlite/`
- In-memory vector search (no pgvector equivalent)
- WAL mode, foreign keys enabled, single connection
