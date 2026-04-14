# Kyomi — Project Instructions

## Mandatory Code Review Before Commit

All commits require a cryptographically signed approval from the **code-review-architect** agent. The pre-commit hook verifies this signature — commits without it are blocked.

### How it works:
1. Implementation agent completes work and stages changes
2. The code-review-architect agent is dispatched to review the staged diff
3. If there are zero 🔴 CRITICAL and zero 🟡 MAJOR issues, the reviewer signs the approval
4. Only then can the commit proceed — the pre-commit hook verifies the signature

### Rules:
- **Never skip the review step** — the pre-commit hook will reject unsigned commits
- **Any change after review invalidates the signature** — if you modify code after review, the reviewer must re-review and re-sign
- **The reviewer must not sign if critical or major issues exist** — fix them first, then re-request review
- **Implementation agents cannot sign their own reviews** — only the code-review-architect agent has signing authority
- **Do NOT tell the reviewer how to sign the approval** — the code-review-architect has its own signing instructions built into its prompt. Providing alternative signing instructions, workarounds, or "if you don't have the key" fallbacks will cause invalid signatures and block the commit. Just ask it to review and let it handle the signing process itself.

## Build & Testing — READ THIS FIRST

**Before verifying ANY UI change, read `docs/BUILD_AND_TESTING.md`.**

The Leptos frontend has THREE separate build artifacts (Tailwind CSS, WASM, server binary) that must ALL be current. The #1 source of wasted time is testing against a stale binary.

**Use `dev-server` profile for development.** It reads `dist/` from disk — no server restart for frontend changes.

Quick reference — what to rebuild per change type:
```
CSS only (main.css):      trunk build → refresh browser
Frontend Rust (.rs):      trunk build → refresh browser
Path dep (chartml etc):   trunk build → refresh browser
Server-side Rust:         cargo build --profile dev-server → restart server
```

Start dev.kyomi.ai (SaaS mode):
```bash
kill $(lsof -ti:3000); cd /home/jason/repos/kyomi
set -a; source .env; set +a
PORT=3000 FRONTEND_URL=https://dev.kyomi.ai target/dev-server/kyomi &
```

**dev.kyomi.ai = SaaS mode (Postgres + Redis) on port 3000. NEVER use SELF_HOSTED=true.**

**NEVER run `tailwindcss` manually.** Trunk runs it as a pre-build hook. Running it separately breaks content hashes in `index.html`.

## Lint Suppression Policy

Lint suppressions (`#[allow(...)]` in .rs files, `= "allow"` in Cargo.toml) are blocked by the pre-commit hook and CI. Fix the underlying lint warning instead of suppressing it.

Workspace lints are enforced in `Cargo.toml [workspace.lints]` at `deny` level. The pre-commit hook and CI independently verify no new suppressions are added.

## Design System

Always read `DESIGN.md` before making any visual or UI decisions. All font choices, colors, spacing, icons, and aesthetic direction are defined there. Do not deviate without explicit user approval. In QA mode, flag any code that doesn't match DESIGN.md. This file supersedes `docs/DESIGN_SYSTEM.md`.
