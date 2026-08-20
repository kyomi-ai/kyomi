# Kyomi — Project Instructions

## Where things live (read this first)

Kyomi is **not** a single repository, and its engineering docs are **not** in this
one. Both facts have caused real defects — an audit concluded a security control
"was never enforced" because it only searched this repo; the enforcement was in
`kyomi-connect`.

### Sibling repositories — search these too

| Repo | Local path | Contains |
|---|---|---|
| **kyomi-connect** | `~/repos/kyomi-connect` | **Datasource drivers and the provider factory** — credential resolution, connection pooling, every per-provider implementation (`crates/kyomi-datasource/`) |
| **chartml** | `~/repos/chartml` | ChartML spec, renderers, chart components |
| **kode** | `~/repos/kode` | The code/WYSIWYG editor |
| **kyomi-private** | `~/repos/kyomi-private` | Deploy infrastructure, k8s, proprietary services — **and `docs/`** (below) |

These are consumed via crates.io (see *External Crate Dependencies*), so their
source is **not** vendored here. When auditing, debugging, or tracing a call that
leaves this workspace, **grep the sibling repo before concluding anything is
missing, unenforced, or unimplemented.** A `grep` limited to `~/repos/kyomi` will
silently miss it.

### Documentation

| Where | What |
|---|---|
| **`~/repos/kyomi-private/docs/`** | **Canonical.** Architecture, per-provider reference, ops runbooks, and the required-reading set. Start at its `README.md`. |
| `docs/product/` (this repo, tracked) | Public-facing product documentation |
| `docs/CODING_STANDARDS.md` (this repo, tracked) | Index over `docs/standards/` — one file per rule (see KYO-375), grouped into section directories. Read the index for the list of sections; `ls docs/standards/<section>/` to see a section's rules. |
| `docs/standards/<section>/` (this repo, tracked) | The coding standards themselves, mined from code reviews — one `README.md` blurb plus one `.md` file per rule. |
| `DESIGN.md` (this repo, tracked) | Design system — visual/UI decisions |
| `docs/*.md` (this repo, **untracked**) | Completed migration plans and historical reports only. `.gitignore` has `docs/*` with `!docs/product/` and `!docs/standards/`, so these exist on one machine and are not authoritative. |

**Do not add new engineering docs to this repo's `docs/`** — they will be
gitignored, invisible to everyone else, and will drift. Put them in
`~/repos/kyomi-private/docs/`. Anything describing *unfixed* weaknesses (open
gaps, vulnerabilities, audit findings) **must** go there: this repo is public.

## Setup

Run `./scripts/setup-hooks.sh` once per clone — it enables the tracked hooks
in `.githooks/`:

| Hook | Enforces |
|---|---|
| `pre-commit` | No new lint suppressions, the server_fn/REST divergence lint (KYO-122), a valid code-review-architect signature |
| `pre-push` | No direct pushes to `main` |

`core.hooksPath` is per-clone git config — it cannot be committed, but the
setting is shared by every worktree of that clone. Because the configured
path is relative (`.githooks`), git resolves it against each worktree's own
top level, so one run correctly activates every worktree's own tracked hooks
(KYO-358). The script is idempotent; re-run it any time you're unsure.

## Build & Testing

The Leptos frontend has THREE separate build artifacts (Tailwind CSS, WASM, server binary) that must ALL be current. The #1 source of wasted time is testing against a stale binary.

**Before verifying ANY UI change, read `~/repos/kyomi-private/docs/BUILD_AND_TESTING.md`.**

**Use `dev-server` profile for development.** It reads `dist/` from disk — no server restart for frontend changes.

Quick reference — what to rebuild per change type:
```
CSS only (main.css):      trunk build → refresh browser
Frontend Rust (.rs):      trunk build → refresh browser
Path dep (chartml etc):   trunk build → refresh browser
Server-side Rust:         cargo build --locked --profile dev-server → restart server
```

**NEVER run `tailwindcss` manually.** Trunk runs it as a pre-build hook. Running it separately breaks content hashes in `index.html`.

**Always pass `--locked` to cargo commands** (`cargo check --locked`, `cargo clippy --locked`, `cargo build --locked`). This prevents silent Cargo.lock drift from transitive dependency re-resolution. If `--locked` fails, run `cargo update` explicitly and commit the lock file as a separate change.

## Sync Engine (Local-First Cache)

**Read `~/repos/kyomi-private/docs/SYNC_ENGINE_ARCHITECTURE.md` before touching any sync/cache code.** That document is the intended authoritative reference — but treat it as *describing intent, not proof*: verify against the code. It fell behind the KYO-172 visibility work and documented the pre-fix (leaking) behaviour; KYO-203 tracks correcting it. Key rules: schema hash gates re-bootstrap on format changes, `session_type = 'chat'` filter on chat sync queries, IDB is a cache not source of truth.

## SSR + Hydration

Some pages are server-side rendered for instant load. **Read `~/repos/kyomi-private/docs/SSR_HYDRATION_GUIDE.md` before touching SSR code.**

Critical rules (violations cause silent hydration panics):
- **Never use `Resource::new()` inside `#[cfg(target_arch = "wasm32")]` blocks** — it desyncs serialized resource IDs between server and client. Use `spawn_local` or `Effect::new` instead.
- **Never inject DOM elements into `<body>` outside the `<App/>` tree** — tachys walks body children and the virtual DOM in lockstep; an extra element causes an immediate panic. Use CSS pseudo-elements for visual indicators.
- **Template splitting must find `<body` after `</head>`** — the string `<body` can appear in CSS comments/selectors. Always search after `</head>`.

## Lint Suppression Policy

Lint suppressions (`#[allow(...)]` in .rs files, `= "allow"` in Cargo.toml) are blocked by the pre-commit hook and CI. Fix the underlying lint warning instead of suppressing it.

Workspace lints are enforced in `Cargo.toml [workspace.lints]` at `deny` level. The pre-commit hook and CI independently verify no new suppressions are added.

## External Crate Dependencies (chartml, kyomi-connect)

Kyomi depends on crates from sibling repos (`chartml`, `kyomi-connect`) via **crates.io**, not path dependencies. Production builds always resolve against the registry.

> **Their source is not in this workspace.** A significant amount of Kyomi's
> behaviour — notably all datasource credential resolution and provider
> construction — lives in `~/repos/kyomi-connect`. See *Where things live* above
> before concluding a control is missing.

**This means fixes in those repos don't reach kyomi until:**
1. The fix is merged to the external repo's main branch
2. A version tag is pushed on that repo to trigger its publish CI (e.g. `v5.0.4` for chartml, `v1.3.2` for kyomi-connect)
3. The new version lands on crates.io
4. Kyomi's `Cargo.lock` is updated: `cargo update <crate-name>`
5. The lock file change is committed and a new kyomi release is cut

Key crates and where they live:

| Crate | Source repo | Kyomi Cargo.toml key |
|-------|-----------|---------------------|
| `chartml-*` | `~/repos/chartml` | `chartml-chart-table = "5.0.4"` etc. |
| `kyomi-datasource` | `~/repos/kyomi-connect` | `kyomi-datasource-drivers = { version = "1.3", package = "kyomi-datasource" }` |
| `kyomi-connect-protocol` | `~/repos/kyomi-connect` | `kyomi-connect-protocol = "1.2"` |
| `kode-leptos` | `~/repos/kode` | `kode-leptos = "0.2"` |

**Local dev** can use `[patch.crates-io]` overrides (see commented examples at the bottom of `Cargo.toml`) to point at local checkouts for faster iteration. These patches are dev-only and are not used in production builds.

## Design System

Always read `DESIGN.md` before making any visual or UI decisions. All font choices, colors, spacing, icons, and aesthetic direction are defined there. Do not deviate without explicit user approval.
