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

## Browser Testing — Verifying UI Changes

**Use Playwright, not the browse/gstack tool.** The browse tool doesn't work reliably with Leptos inputs and can't load debug WASM (253MB, 8-15s load time).

### Release WASM for Playwright

Debug WASM is too large for Playwright timeouts. Build release WASM first:

```bash
cd crates/kyomi-ui
RUSTUP_TOOLCHAIN=nightly trunk build --release && gzip -9 -k dist/*_bg.wasm
```

### Seed test users (one-time)

```bash
python3 scripts/e2e-regression/seed-test-user.py
# Requires: pip3 install argon2-cffi psycopg2-binary
```

### Test credentials

| Email | Password | Role |
|-------|----------|------|
| `e2e-test@kyomi.dev` | `E2eTestPass123!` | Regular user |
| `e2e-admin@kyomi.dev` | `E2eAdminPass123!` | Admin user |

### Playwright login flow

```javascript
const { chromium } = require('playwright');
const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
const page = await ctx.newPage();
await page.goto('http://localhost:3000/login', { waitUntil: 'networkidle', timeout: 15000 });
await page.fill('input[type="email"]', 'e2e-test@kyomi.dev', { timeout: 8000 });
await page.fill('input[type="password"]', 'E2eTestPass123!', { timeout: 8000 });
await page.click('button[type="submit"]', { timeout: 8000 });
await page.waitForURL(url => !url.toString().includes('/login'), { timeout: 15000 });
// Now authenticated — navigate to the affected page
```

### Playwright rules

- Scripts MUST use `.cjs` extension (repo has `"type": "module"`)
- Run with: `NODE_PATH=/home/jason/repos/kyomi/node_modules node /path/to/script.cjs`
- Screenshots: full page, 1920x1080, saved to `/tmp/`

## Lint Suppression Policy

Lint suppressions (`#[allow(...)]` in .rs files, `= "allow"` in Cargo.toml) are blocked by the pre-commit hook and CI. Fix the underlying lint warning instead of suppressing it.

Workspace lints are enforced in `Cargo.toml [workspace.lints]` at `deny` level. The pre-commit hook and CI independently verify no new suppressions are added.

## Linear Ticket References in PRs

When a PR closes Linear tickets, reference them so the Linear GitHub integration
auto-links the PR to each issue and moves them to Done on merge.

**Rules:**

1. **Branch name** — use the Linear-suggested format `jason/kyo-NN-short-slug` (copy
   it from the ticket's "Copy git branch name" button). One ticket per branch is the
   happy path; the integration auto-links as soon as the branch is pushed. For
   multi-ticket batched PRs, use the *primary* ticket in the branch name.
2. **PR body** — add each ticket on its own line at the top, not comma-separated:

   ```
   Closes KYO-19
   Closes KYO-20
   Closes KYO-21
   ```

   Linear's parser is more reliable with one-per-line than with comma-separated
   references. Comma-separated works *sometimes* but tends to attach the PR to
   only the first ID.
3. **Commit message** — the same `Closes KYO-NN` lines go in the commit body so
   squash-merge preserves the references. One per line, not comma-separated.
4. **Manual fallback** — if the auto-link didn't attach the PR to every ticket
   after merge, move the missing ones to Done via the Linear API and link the PR
   as an attachment. Don't leave them in Backlog when the work is live.

## Design System

Always read `DESIGN.md` before making any visual or UI decisions. All font choices, colors, spacing, icons, and aesthetic direction are defined there. Do not deviate without explicit user approval. In QA mode, flag any code that doesn't match DESIGN.md. This file supersedes `docs/DESIGN_SYSTEM.md`.
