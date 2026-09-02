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
  `scripts/check-ticket-in-flight.sh <TICKET> [--remote <name>] [--ignore-branch <name>]... [--self <branch>]`,
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
  **`--self <branch>` makes self-exclusion cwd-independent (KYO-593):** the
  script's default self-exclusion is derived from
  `git rev-parse --abbrev-ref HEAD` in the invoking shell's cwd, which is
  right at pickup (cwd is the canonical clone the worker claims the ticket
  from) but wrong for the pre-review call if the caller's implementation by
  then lives in a worktree while its shell is still sitting elsewhere —
  cwd-derived exclusion then resolves to the wrong branch (often `main`,
  which the script already discards) and the caller's own finished work
  reads as somebody else's. `--self <branch>` names the caller's own ticket
  branch explicitly instead of inferring it from cwd, so the check works
  from any directory. Unlike `--ignore-branch`, which is an unvalidated
  operator escape hatch, `--self` **is validated** against the ticket
  (`matches_ticket`, exit `2` if it doesn't match) — it changes only *how*
  the caller's own branch is identified, never adding suppression power
  `--ignore-branch` doesn't already have, so it cannot become a silencer for
  a genuine competitor's branch. **Operational rule:** the pickup call needs
  no flag (cwd-derived exclusion is correct there); the pre-review call
  should pass `--self "$BRANCH"` so it works regardless of which directory
  it happens to run from.
  **Honours `STRANDED.md` tombstones (KYO-529):** the local-worktree check
  and the local-branch check for /backlog-fast Step 0.5's stranded-claim
  recovery deadlocked each other — Step 0.5 deliberately preserves a dead
  run's worktree for salvage, and a preserved dead worktree is otherwise
  indistinguishable from a live worker's, so the ticket went back to
  Backlog looking available and was never picked up again (KYO-448 twice,
  KYO-463). A worktree matching the ticket whose root holds a valid
  tombstone — written by `mark-worktree-stranded.sh`, below — is reported
  separately as *preserved*, not counted as a hit, and printed under its
  own heading (`PRESERVED STRANDED WORK`) on every verdict so the
  unsalvaged work is never silently forgotten. `STRANDED.md` only ever
  suppresses local evidence: it never suppresses a PR, and never suppresses
  a remote branch by itself.
  **Also honours `stranded/` remote and local branches (KYO-567):** a
  worker that gets as far as `git push` and then dies leaves a pushed
  branch with no PR forever — check 1 previously reported it in flight
  permanently, since there is no PR for /merge-sweeper to close and Step
  0.5's stranded check never reaches it. `mark-branch-stranded.sh`, below,
  renames such a branch on the remote (and locally, when safe) to
  `stranded/<branch>`. Unlike a local `STRANDED.md`, a `stranded/` ref
  lives on the remote itself and can only be created by something with
  push access, so it clears the bar to suppress a *remote*-branch hit too
  (check 1), not just local evidence — see the script's own header for the
  durability rule this follows. It still never suppresses a PR hit (check
  2); `mark-branch-stranded.sh` itself refuses to tombstone any branch that
  has one. Self-tested by `scripts/check-ticket-in-flight-test.sh`,
  including a real simulated double-pickup against a throwaway bare git
  remote (the KYO-422 acceptance criterion), real `git worktree add`
  fixtures for the `STRANDED.md` cases, an interop check that a ref
  `mark-branch-stranded.sh` actually created is honoured here, and (KYO-593)
  a fixture that reproduces the cwd-dependence bug from a checkout sitting
  on `main` and shows `--self` clears it while a second worker's worktree
  for the same ticket still blocks.

- **`mark-worktree-stranded.sh`** - The writer side of the KYO-529 tombstone
  above: writes `STRANDED.md` at a preserved worktree's root so
  `check-ticket-in-flight.sh` stops reading it as a live claim. Keeping the
  marker's format in one script, rather than restating it in prose across
  the skill files that call it, follows the same KYO-422 principle the
  check script itself is built on. Usage:
  `scripts/mark-worktree-stranded.sh <TICKET> [--worktree <path>] [--note <text>]`
  — `TICKET` accepts `KYO-529`, `kyo-529`, or `529`; `--worktree` defaults
  to the current worktree's root; `--note` is optional free text (why the
  run died, what's left to salvage) appended to the marker body. Refuses to
  write to the primary worktree (compares `git rev-parse --git-dir` against
  `--git-common-dir`) and refuses a worktree on branch `main` or in
  detached HEAD — writing this file at the canonical clone's own root, or
  tombstoning a state this workflow never produces, would be a bad day.
  Overwriting an existing marker is allowed and idempotent (a stderr
  warning says so). **Exit-code contract:** `0` success; `1` error
  (primary worktree, branch `main`/detached HEAD, git predates 2.31, or the
  write itself failed); `2` usage error. Self-tested by
  `scripts/mark-worktree-stranded-test.sh`, including an interop check that
  `check-ticket-in-flight.sh` actually honours a marker this script wrote.

- **`mark-branch-stranded.sh`** - The writer side of the KYO-567 `stranded/`
  remote-branch tombstone above, and the answer to "what does *releasing* a
  ticket whose worker died after `git push` actually do?" It is
  deliberately NOT "open a PR" — see the script's own header for why that
  was investigated and rejected on evidence (both reproduction branches for
  KYO-534/KYO-463 contained only a mined-coding-standards commit, never the
  ticket's actual work, so a `Closes KYO-NNN` PR opened at release time
  would hand `/merge-sweeper` a green PR that marks a real bug Done — a
  silent false completion). Instead it renames `<branch>` to
  `stranded/<branch>` on the remote: verifies the new ref exists and points
  at the same sha *before* deleting the original (never destroys before the
  copy is confirmed — any failed step aborts leaving the original ref
  exactly as it was), and does the symmetric local rename when the branch
  isn't checked out anywhere. Usage:
  `scripts/mark-branch-stranded.sh <TICKET> --branch <name> [--remote <name>] [--note <text>]`
  — `TICKET` accepts `KYO-567`, `kyo-567`, or `567`; `--branch` is required;
  `--remote` defaults to `origin`; `--note` is optional free text folded
  into the printed summary. Refuses branch `main`; refuses any branch that
  has a PR in any state (matched via `gh pr list --head <branch>` — that
  branch belongs to `/merge-sweeper`, not this script); is idempotent on a
  branch already under `stranded/` or on a retry after a prior run already
  completed; and if the local branch is checked out in some *other*
  worktree, refuses unless that worktree already carries its own valid
  `STRANDED.md` for the same ticket (printing the exact
  `mark-worktree-stranded.sh` command to run first) — composing the two
  tombstone mechanisms rather than leaving a local claim unexplained.
  **Exit-code contract:** `0` tombstoned (or already tombstoned — this
  script is idempotent); `1` error (main, has a PR, worktree checkout
  lacks a tombstone, branch missing on the remote, any git/gh step failed,
  or a post-push verification mismatch); `2` usage error. Self-tested by
  `scripts/mark-branch-stranded-test.sh`, including the happy path against
  a throwaway bare remote, both PR and worktree-tombstone refusals, both
  idempotency shapes, and — the load-bearing case — that a failed push
  (forced via a pre-existing conflicting `stranded/` ref) leaves the
  original remote ref completely untouched.

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
