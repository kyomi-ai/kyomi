# Kyomi Scripts

All scripts are organized by environment. **Every script is environment-specific** - no ambiguity about which database/services they target.

## Repo setup

- **`setup-hooks.sh`** - Run once per clone to enable the tracked git hooks
  in `.githooks/`; the setting is shared with every worktree of that clone.
  See root `CLAUDE.md` "Setup".

- **`mine-review-logs.sh`** - Locates recent code-review logs for the
  coding-standards-mining step (KYO-373 / KYO-386), from any worktree.
  `docs/review-logs/` is **intentionally untracked and machine-local** —
  `.gitignore` excludes `docs/*` except `docs/product/`, because review
  logs are audit findings (some describing still-open issues) that must
  not be published from this public repo. That means the directory only
  exists in whichever clone has actually accumulated review runs; it is
  never present in a fresh clone and is invisible to any linked worktree
  unless this script is used to find it. Run it as
  `scripts/mine-review-logs.sh [days]` (default `7`, must be a positive
  integer) — it walks up to the canonical clone via
  `git rev-parse --path-format=absolute --git-common-dir`, so it works
  identically from the main checkout or any worktree. Matching log paths
  (filtered by the `YYYY-MM-DD.md` filename, not mtime) print one per line
  on stdout, oldest to newest; human-facing status goes to stderr.
  **Exit-code contract:** `0` means `docs/review-logs/` was found (the
  stdout list may legitimately be empty — a quiet review week is not an
  error); `2` means the directory doesn't exist in the canonical clone at
  all, which is the loud-failure signal a caller must report as an
  explicit skip rather than silently continuing past; `1` is a usage error
  (bad argument, or not run inside a git repository).

## Directory Structure

```
scripts/
└── dev/          # Development environment scripts
    └── dangerous/    # Risky dev operations (data loss, resets)

## Development Scripts (`scripts/dev/`)

All dev scripts use `.env` and target:
- PostgreSQL: localhost:5433
- Redis: localhost:6380
- Backend: localhost:8002
- Frontend: localhost:5173

### Service Management
- **`start.sh`** - Start all dev services (PostgreSQL, Redis, backend, frontend)
- **`stop.sh`** - Stop all dev services
- **`restart.sh`** - Restart all dev services
- **`start-backend.sh`** - Start just the backend API
- **`start-frontend.sh`** - Start just the frontend dev server
- **`start-services.sh`** - Start just PostgreSQL + Redis containers
- **`stop-services.sh`** - Stop PostgreSQL + Redis containers
- **`restart-backend.sh`** - Restart backend API
- **`restart-frontend.sh`** - Restart frontend dev server

### Database Management
- **`setup-database.sh`** - Initialize database schema
- **`migrate-database.sh`** - Run database migrations

### Utilities
- **`populate-cache.sh`** - Populate BigQuery cache in dev database
- **`build-frontend.sh`** - Build frontend in dev mode

### Dangerous Operations (`scripts/dev/dangerous/`)
⚠️ **These can cause data loss or major changes**
- **`reset-database.sh`** - Wipe and rebuild database (ALL DATA LOST!)
- **`reset-tours.sh`** - Reset UI tours for all users
- **`export-beta-signups.sh`** - Export beta signup emails

## Quick Start

### Development

```bash
# Start everything
./scripts/dev/start.sh

# Or start services individually
./scripts/dev/start-services.sh  # PostgreSQL + Redis
./scripts/dev/start-backend.sh   # Backend API
./scripts/dev/start-frontend.sh  # Frontend dev server

# Setup database (first time)
./scripts/dev/setup-database.sh

# Stop everything
./scripts/dev/stop.sh
```

## Key Principles

1. **Every script is environment-specific** — no ambiguity about which database/services they target
2. **Dev scripts run on host** — use `.env`, connect to localhost:5433 (PostgreSQL), localhost:6380 (Redis)

Update any automation or documentation that references old paths.
