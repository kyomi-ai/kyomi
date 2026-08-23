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

- **`append-review-log.sh`** - The write-side counterpart to
  `mine-review-logs.sh` (KYO-396): appends a code-review entry (read from
  stdin) to the canonical daily review log, from any worktree, so the
  location is a property of the tooling instead of agent compliance with
  an instruction (KYO-387 fixed the same failure by rewriting an
  instruction in `~/.claude/agents/code-review-architect.md`; that held
  only as long as every invocation followed it, and several didn't —
  KYO-371, KYO-397). Run it as `some-command | scripts/append-review-log.sh`
  or with a heredoc; it takes no arguments and appends to
  `<canonical-root>/docs/review-logs/$(date +%F).md`, creating the
  directory and/or file if absent. Concurrent-ish appends (e.g. two review
  cycles for the same ticket, running from different worktrees) are
  serialized with `flock` so entries never interleave and always land in
  one file in call order. **Exit-code contract:** `0` success; `2` means
  stdin was empty (or whitespace-only) — refused rather than writing a
  blank entry; `1` is a usage/environment error (unexpected arguments, not
  inside a git repository, or git older than 2.31). Self-tested by
  `scripts/append-review-log-test.sh`, including a worktree-cwd
  reproduction of the exact gap KYO-396 was filed to close.

- **`lib/canonical-root.sh`** - Shared helper, meant to be `source`d rather
  than run directly. Provides `resolve_canonical_root` and
  `resolve_review_logs_dir`, used by both `mine-review-logs.sh` (read side)
  and `append-review-log.sh` (write side) so the
  `git rev-parse --path-format=absolute --git-common-dir` resolution and
  the on-disk review-log path exist in exactly one place. `resolve_review_logs_dir`
  is the single point of change if KYO-394 moves the canonical location.

- **`check-ticket-in-flight.sh`** - Answers one question before an
  autonomous worker claims a backlog ticket: *is anyone else already
  working on this?* (KYO-422, fixing the double-pickup of KYO-416 that
  produced conflicting PRs #367/#368). Checks remote branches
  (`git ls-remote --heads`), pull requests (`gh pr list`, matched on
  `headRefName` only — never PR body or title, see the script header for
  why that was tried and reverted, KYO-471), local worktrees
  (`git worktree list`), and local branches (`git branch --list`). The
  last two exist because a worker whose run dies between `git commit` and
  `git push` leaves a complete implementation visible only locally
  (KYO-471). Run it **twice** per ticket: once at pickup, and again
  immediately before dispatching code review — the second call is what
  catches two workers who were both already past the claim point when
  they started, which is exactly how KYO-416 happened. Usage:
  `scripts/check-ticket-in-flight.sh <TICKET> [--remote <name>] [--ignore-branch <name>]...`,
  where `TICKET` is `KYO-422`, `kyo-422`, or `422` (equivalent). **Exit-code
  contract:** `0` — clear, **the only code that permits claiming the
  ticket**; `1` — work in flight found, do not claim; `2` — usage error;
  `3` — a check could not be completed (remote unreachable, `gh` missing
  or failing, **or the PR listing came back at `--limit` and may therefore
  be truncated**) and must be treated exactly like `1`, never like `0` —
  the script fails closed by design, since a false "clear" costs a full
  duplicate implementation while a false "in flight" costs one skipped
  cycle. The PR page size is the env-overridable `PR_LIST_LIMIT` (default
  `500`, must exceed the repo's total PR count — it was 411 on 2026-08-24);
  raising it is the fix when the truncation guard trips, and the guard is
  why raising it is a deliberate act rather than a silently wrong answer.
  Self-tested by `scripts/check-ticket-in-flight-test.sh`, including a real
  simulated double-pickup against a throwaway bare git remote (the KYO-422
  acceptance criterion).

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
