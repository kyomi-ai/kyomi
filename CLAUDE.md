# Kyomi — Project Instructions

## Build & Testing

The Leptos frontend has THREE separate build artifacts (Tailwind CSS, WASM, server binary) that must ALL be current. The #1 source of wasted time is testing against a stale binary.

**Before verifying ANY UI change, read `docs/BUILD_AND_TESTING.md`.**

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

**Read `docs/SYNC_ENGINE_ARCHITECTURE.md` before touching any sync/cache code.** That document is the authoritative reference — if the code doesn't match it, the code is wrong. Key rules: schema hash gates re-bootstrap on format changes, `session_type = 'chat'` filter on chat sync queries, IDB is a cache not source of truth.

## SSR + Hydration

Some pages are server-side rendered for instant load. **Read `docs/SSR_HYDRATION_GUIDE.md` before touching SSR code.**

Critical rules (violations cause silent hydration panics):
- **Never use `Resource::new()` inside `#[cfg(target_arch = "wasm32")]` blocks** — it desyncs serialized resource IDs between server and client. Use `spawn_local` or `Effect::new` instead.
- **Never inject DOM elements into `<body>` outside the `<App/>` tree** — tachys walks body children and the virtual DOM in lockstep; an extra element causes an immediate panic. Use CSS pseudo-elements for visual indicators.
- **Template splitting must find `<body` after `</head>`** — the string `<body` can appear in CSS comments/selectors. Always search after `</head>`.

## Lint Suppression Policy

Lint suppressions (`#[allow(...)]` in .rs files, `= "allow"` in Cargo.toml) are blocked by the pre-commit hook and CI. Fix the underlying lint warning instead of suppressing it.

Workspace lints are enforced in `Cargo.toml [workspace.lints]` at `deny` level. The pre-commit hook and CI independently verify no new suppressions are added.

## Design System

Always read `DESIGN.md` before making any visual or UI decisions. All font choices, colors, spacing, icons, and aesthetic direction are defined there. Do not deviate without explicit user approval.
