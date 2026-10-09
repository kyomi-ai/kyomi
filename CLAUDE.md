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
| `docs/*.md` (this repo, **untracked by default**) | Completed migration plans and historical reports only. `.gitignore` has `docs/*` with `!docs/product/` and `!docs/standards/`, so these exist on one machine and are not authoritative — except `docs/CODING_STANDARDS.md`, which is tracked (see the row above) as an explicit exception to this default. |
| **`~/repos/kyomi-private/skills/`** | **Canonical.** The agent-workflow skill/build-test docs (`SKILL.md` for `backlog`/`backlog-fast`, and `build-test.md`) — see KYO-568. This repo's `.claude/build-test.md` is meant to be a symlink into it (via `kyomi-private/scripts/link-agent-skills.sh`); edit it there and open a PR in `kyomi-private`, not here. |

**Do not add new engineering docs to this repo's `docs/`** — they will be
gitignored, invisible to everyone else, and will drift. Put them in
`~/repos/kyomi-private/docs/`. Anything describing *unfixed* weaknesses (open
gaps, vulnerabilities, audit findings) **must** go there: this repo is public.

## Fast backlog handoff

`/backlog-fast` implements, obtains review, opens a PR, and waits and fixes failures
until the current head's CI is green. It never merges, including under `/loop` and
cron; it leaves the ticket **In Review**. `/merge-sweeper` owns the merge and the
**Done** transition, using its documented `/usr/bin/gh` path without a
test-verification signature for handed-off fast PRs. Invoking `/merge-sweeper`
authorizes that path. Do not stop mid-loop to ask which merge path to use.

The canonical versioned workflows are
`~/repos/kyomi-private/skills/backlog-fast/SKILL.md` and
`~/repos/kyomi-private/skills/merge-sweeper/SKILL.md`. Build and browser QA remain
batched against `main` after merging.

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

### Review after a rebase with no staged change

The pre-commit hook checks a staged approval when a commit is created. A
rebase can rewrite already reviewed commits without invoking that hook. Before
pushing a rebased branch, give the code-review-architect the committed range
from the current base to `HEAD` and ask it to review that exact range. After
its approval, the reviewer signs it with:

```bash
bash scripts/sign-review.sh "<reviewer private key>" --committed-range origin/main
```

The reviewer should run this only after reading the range and confirming the
worktree is clean. The approval is recorded in the ignored
`.review-range-approval` file, with the base commit, `HEAD` commit, and diff
hash bound into the signature. Before **each** push of a rebased branch, run:

```bash
git fetch origin main && bash scripts/sign-review.sh --verify-committed-range origin/main
```

Both commands must succeed; if the fetch fails, stop and do not push. Fetch
immediately before verification so a stale local `origin/main` cannot make an
old approval appear current. A further rebase, a changed `HEAD`, or a new
`origin/main` base invalidates the approval and requires another review and
signature. This is an explicit review-time gate: `pre-push` only blocks direct
pushes to `main` and does not enforce committed-range approval. Ordinary
staged changes still use the existing one-argument signing call and
`.review-approval` pre-commit gate.

## Rust toolchain

`rust-toolchain.toml` is the single source of truth for normal development,
blocking CI, server/Docker and desktop releases, and the release/LTO smoke test.
Run Cargo from inside this checkout with rustup installed; rustup selects and
installs the exact release, Clippy, rustfmt and wasm32 target automatically.
Clear `RUSTUP_TOOLCHAIN` and directory overrides when running normal checks.
CI uses `.github/actions/setup-rust` to read the file explicitly, because
`dtolnay/rust-toolchain@stable` would override it. Jobs print compiler/Cargo
versions and use a compiler/job-specific sccache namespace. Docker's scratch
image packages artifacts compiled by that pinned release workflow.

The initial pin is the stable release published on 2026-10-01 (the exact number
lives only in the toolchain file). Jason originally chose rolling stable around
six months earlier for a required feature absent from an older compiler; that
feature was not identified in the recorded discussion. Preserve that capability
by selecting the current stable release and validating the existing feature
matrix rather than assuming an old compiler suffices. The workspace uses Rust
2024 and resolver 3; hydration builds use the stable WASM shims, not nightly
`build-std`. This pin defines the build compiler, **not a new MSRV**. Verification
must record actual platform run links and versions; selecting a release alone
is not proof the supported matrix passes. Initial local Linux verification on
2026-10-02 passed `cargo check --workspace --locked` using the pin, compiling the
native workspace including server and Linux desktop in 5m41s. Clippy, WASM
artifacts and platform release validation are separate results recorded in the PR.

`.github/workflows/rust-stable-canary.yml` runs at 03:17 UTC nightly and on
manual dispatch. It intentionally selects `+stable` (and `RUSTUP_TOOLCHAIN=stable`
for Trunk subprocesses), independently runs all three Clippy configurations
read from `ci.yml`, and keeps compiler/configuration caches separate. A failed
canary remains red with summaries and job-log links; it has no PR trigger and
must remain outside required branch-protection checks. `cargo audit` continues
to use the current advisory database and fail on findings under its existing
policy; its existing non-required merge status is unchanged.

Upgrade procedure:

1. Read all three latest-stable canary results, including setup failures and
   uploaded diagnostics. Run it on demand with
   `/usr/bin/gh workflow run rust-stable-canary.yml --repo kyomi-ai/kyomi`.
2. Change only the exact release in `rust-toolchain.toml` in a reviewed PR.
   Resolve new diagnostics together; retain `-D warnings` and avoid suppressions
   or downgrades to evade warnings.
3. Run `cargo check --workspace --locked` and `scripts/preflight-clippy.sh`.
   Run normal CI for native checks/tests, the three Clippy gates and WASM
   hydration. Preserve workflow/preflight parity self-tests.
4. Validate production server/Docker and standalone Linux x86_64 builds
   with `release.yml` on the candidate branch: manual dispatch builds artifacts
   without publishing images, releases or deployments. Validate Linux/macOS/
   Windows desktop with branch dispatch of `desktop.yml` (only tag runs attach
   installers). Run the release-profile LTO smoke test on the same branch.
   Never create a release tag merely to validate a toolchain.
5. Record run links, installed compiler/Cargo versions, cache namespaces and
   genuine platform limitations in the PR. Merge after supported checks pass.

To demonstrate canary failure before merging a new workflow, use a temporary
verification branch with a temporary PR trigger restricted to that branch's
PR, then inject a failing command **in one Clippy matrix configuration** after
setup. Review that temporary diff before pushing. Open a temporary PR, verify
all three configurations ran independently, the injected configuration is red,
and its summary identifies the failure with compiler versions and log links.
Verify the ordinary CI check names/triggers and remote required checks are
unchanged. Close that temporary PR and remove the injection/temporary trigger
before delivery. New `workflow_dispatch` workflows may require registration
on the default branch; after registration use manual dispatch instead. Never
add a permanent PR trigger or a required check for the canary.

## Build & Testing

The Leptos frontend has THREE separate build artifacts (Tailwind CSS, WASM, server binary) that must ALL be current. The #1 source of wasted time is testing against a stale binary.

**Before verifying ANY UI change, read `~/repos/kyomi-private/docs/BUILD_AND_TESTING.md`.**

**Use `dev-server` profile for development.** It reads `dist/` from disk — no server restart for CSR frontend changes (static assets, the WASM bundle). The SSR-rendered `/login` page is the exception: it's compiled into the server binary itself, not read from `dist/`.

Quick reference — what to rebuild per change type:
```
CSS only (main.css):      trunk build → refresh browser
Frontend Rust (.rs):      trunk build → refresh browser; ALSO cargo build --locked --profile dev-server → restart server if the change touches code the SSR login page renders
Path dep (chartml etc):   trunk build → refresh browser
Server-side Rust:         cargo build --locked --profile dev-server → restart server
```

**`kyomi-ui` is statically linked into `apps/server`, not just served as static files.** `login_ssr_handler` (`apps/server/src/leptos_frontend.rs`) calls `kyomi_ui::app::App` directly to server-render `/login`, so a Rust change to `LoginPage` or to a provider that always wraps the router (`ThemeProvider`, `ToastProvider`, `NavigationProgress`) changes the *server-rendered* markup too — and `trunk build` alone won't update it, because that only rebuilds the client WASM bundle. Rebuild the server binary and restart whenever a frontend Rust change could reach the login page's render tree.

**NEVER run `tailwindcss` manually.** Trunk runs it as a pre-build hook. Running it separately breaks content hashes in `index.html`.

**Always pass `--locked` to cargo commands** (`cargo check --locked`, `cargo clippy --locked`, `cargo build --locked`). This prevents silent Cargo.lock drift from transitive dependency re-resolution. If `--locked` fails, run `cargo update` explicitly and commit the lock file as a separate change.

**The workspace is not rustfmt-clean**, so `cargo fmt --check` cannot pass (~384 files fail on `main` @ `fcef65eb`) and is not a verification step. **Never run bare `cargo fmt`** — including `cargo fmt -- <files>`, which does not limit the blast radius — it rewrites the entire workspace. To format a single file, use `rustfmt --edition 2024 <path>` (bare `rustfmt` defaults to edition 2015 and errors on `async fn`; match the crate's own edition — 15 of 16 crates here are `2024`), noting it also reformats every module the target file declares with `mod`, so aim it at the leaf file you edited, not a crate root. Or match the surrounding style by hand.

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
