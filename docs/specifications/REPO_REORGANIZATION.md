# Repo Reorganization — Design Specification

## Overview

Flatten the nested `apps/backend-rust/` structure, promote the Cargo workspace to repo root, and clean out dead files. Each product (server, desktop, frontend) gets an obvious home, shared Rust crates move to a top-level `crates/` directory, and nothing is buried three levels deep.

## Current Problems

1. **`apps/backend-rust/` nesting** — every Rust file is at `apps/backend-rust/crates/kyomi-foo/src/bar.rs` (4 levels before code). The Cargo workspace sits inside `apps/` alongside frontend, which makes no sense given Rust is 80% of the codebase.

2. **Naming confusion** — `kyomi-api` crate produces a binary called `kyomi`. It's not an API library, it's the full server application.

3. **Dead artifacts at root** — 1.1GB backup dump tracked in git, empty `apps/backend-python/`, 7.3GB `.venv/`, empty `postgres_data/`, stale test artifacts.

4. **Scattered shared data** — `shared/constants.toml`, `data/chartml-spec/`, `config/development.json` are all different kinds of shared data in different places.

5. **Private infra in public repo** — `apps/analytics-collector/` is Kyomi Cloud infrastructure, not needed by self-hosted or open source users.

## Proposed Structure

```
kyomi/
├── Cargo.toml               ← ROOT workspace (moved from apps/backend-rust/)
├── Cargo.lock
│
├── apps/
│   ├── server/              ← kyomi-api crate renamed, produces `kyomi` binary
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   ├── migrations/
│   │   ├── migrations-sqlite/
│   │   └── tests/
│   ├── desktop/             ← kyomi-desktop crate (Tauri app)
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   ├── icons/
│   │   └── tauri.conf.json
│   ├── frontend/            ← unchanged (React/Vite SPA)
│   ├── chart-renderer/      ← unchanged (Enterprise, server-side PNG)
│   └── mcp-chart-app/       ← unchanged (single-file chart viewer)
│
├── crates/                  ← shared Rust libraries
│   ├── kyomi-core/
│   ├── kyomi-auth/
│   ├── kyomi-agent/
│   ├── kyomi-embed/
│   ├── kyomi-datasource/
│   └── kyomi-knowledge/
│
├── enterprise/              ← proprietary-licensed code
│   ├── kyomi-slack/
│   └── LICENSE
│
├── packages/                ← JS packages (unchanged)
│   ├── chart-header/
│   ├── chartml-core/
│   ├── chartml-data-bigquery/
│   └── chartml-transform/
│
├── data/                    ← embedded data files
│   ├── chartml-spec/        ← unchanged
│   └── constants.toml       ← moved from shared/
│
├── deploy/                  ← self-hosted docker-compose files
├── k8s/                     ← k8s manifests
├── scripts/                 ← dev scripts
├── docs/                    ← documentation
│
├── Dockerfile.standalone
├── docker-compose.dev.yml
├── package.json             ← root (playwright tests)
├── VERSION
├── LICENSE
├── LICENSE-COMMERCIAL.md
├── CLA.md
├── NOTICE
└── README.md
```

## Key Changes

### 1. Move Cargo workspace to repo root

`apps/backend-rust/Cargo.toml` → `Cargo.toml` (root). `Cargo.lock` moves to root.

Current path to code: `apps/backend-rust/crates/kyomi-core/src/config.rs`
New path: `crates/kyomi-core/src/config.rs`

Workspace members become:
```toml
[workspace]
members = [
    "crates/kyomi-core",
    "crates/kyomi-auth",
    "crates/kyomi-embed",
    "crates/kyomi-datasource",
    "crates/kyomi-agent",
    "crates/kyomi-knowledge",
    "apps/server",
    "apps/desktop",
    "enterprise/kyomi-slack",
]
```

### 2. Rename kyomi-api → server

The `kyomi-api` crate moves from `apps/backend-rust/crates/kyomi-api/` to `apps/server/`. The crate name in `Cargo.toml` becomes `kyomi-server`. The `[[bin]]` name stays `kyomi` (that's what users type).

### 3. Move desktop crate

`apps/backend-rust/crates/kyomi-desktop/` → `apps/desktop/`

### 4. Move shared crates

`apps/backend-rust/crates/kyomi-*` → `crates/kyomi-*` (6 library crates)

### 5. Move enterprise

`apps/backend-rust/enterprise/` → `enterprise/`

### 6. Consolidate shared data

- `shared/constants.toml` → `data/constants.toml`
- Delete `shared/` directory
- `config/development.json` → `scripts/dev/development.json` (if used) or delete

### 7. Delete dead files and directories

| Item | Action |
|------|--------|
| `apps/backend-python/` | Delete — empty deprecated snapshot |
| `apps/analytics-collector/` | Delete — private cloud infra, lives in private repo |
| `apps/docs/` | Delete if empty/unused |
| `backups/kyomi_*.sql` | Remove from git tracking, add to .gitignore |
| `kyomi_backup_*.dump` at root | Delete locally, already gitignored |
| `shared/` | Move constants.toml to data/, delete dir |
| `config/` | Move or delete contents, delete dir |
| `marketing/` | Move `kyomi-connect-landing.html` to docs/ or delete |
| `MANUAL_TESTING_CHECKLIST.md.backup` | Delete |
| `clickhouse/` at root | Move to `scripts/dev/` if dev config, or delete |
| `logs/` at root | Ensure gitignored |

### 8. Delete apps/backend-rust/

After all contents are moved, `apps/backend-rust/` is deleted entirely. No more nesting.

## What Does NOT Change

- **`apps/frontend/`** — stays as-is
- **`packages/`** — stays as-is
- **`deploy/`**, **`k8s/`**, **`scripts/`**, **`docs/`** — stay as-is
- **Crate names** (except kyomi-api → kyomi-server) — all `use kyomi_core::...` imports stay the same
- **Binary names** — `kyomi` (server), `kyomi-desktop` (desktop)
- **Module structure** within each crate — no internal refactoring

## Path Updates Required

These files reference `apps/backend-rust/` paths and need updating:

| File | What changes |
|------|-------------|
| `Dockerfile.standalone` | COPY paths, WORKDIR |
| `.github/workflows/*.yml` | Build paths, cargo commands |
| `scripts/dev/start-rust-backend.sh` | Working directory |
| `docker-compose.dev.yml` | Volume mounts, build context |
| `apps/server/src/frontend.rs` | `rust_embed` folder path |
| `apps/server/src/routes/mcp.rs` | `include_str!()` path for chart_app.html |
| `crates/kyomi-agent/src/prompt.rs` | `include_str!()` path for chartml-spec |
| `crates/kyomi-core/src/constants.rs` | Path to constants.toml |
| `apps/desktop/Cargo.toml` | Path dependency to kyomi-api → apps/server |
| All `Cargo.toml` files | `path = ` references between crates |

## Implementation Approach

This is a mechanical move — no code logic changes. Do it in one commit:

1. Move all directories to new locations
2. Update root `Cargo.toml` workspace
3. Update all `Cargo.toml` path references
4. Update `include_str!()` and `rust_embed` paths
5. Update Dockerfile, CI workflows, scripts
6. Delete dead files
7. Run `cargo check --workspace` to verify
8. Run frontend build to verify
9. Single commit: "refactor: reorganize repo structure — promote Cargo workspace to root"

## Risks

1. **CI/CD paths** — every workflow references `apps/backend-rust/`. Must update all of them.
2. **Private repo** — the private deploy repo references paths from this repo. Needs coordinated update.
3. **Worktrees** — existing worktrees (like `kyomi-desktop`) will break. Clean them up before the move.
4. **Developer muscle memory** — paths change. One-time cost.
