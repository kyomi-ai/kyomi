# Build & Testing Guide

**The #1 source of wasted time: testing against a stale binary.**

This document exists because we keep making the same mistakes. Read it before verifying any UI change.

---

## Verification Quickstart

**Every agent verifying a PR runs this checklist top-to-bottom before doing anything else.** Each step is a check. If it passes, move on. If it fails, run the fix command, then re-check.

Every verifier run happens in its own **git worktree** on a **dedicated port**. This keeps verification isolated from the global `dev.kyomi.ai` instance on `:3000` and lets multiple agents verify different PRs in parallel.

**Your dispatch prompt should name:**
- `WORKTREE` — absolute path to the worktree (e.g. `/home/jason/repos/kyomi-wt-kyo-123-slug`)
- `PORT` — the port assigned for this worktree's dev server (3100–3999)

If the orchestrator did not give you a `WORKTREE`/`PORT`, fall back to:
1. Create the worktree yourself: `git worktree add /home/jason/repos/kyomi-wt-pr-<N> $(gh pr view <N> --json headRefName -q .headRefName)`
2. Symlink `.env`: `ln -s /home/jason/repos/kyomi/.env <WORKTREE>/.env`
3. Pick a free port in 3100–3999 (try `3000 + (PR_NUMBER % 900)` first).

Export both so every subsequent command picks them up:

```bash
export WORKTREE=/home/jason/repos/kyomi-wt-kyo-123-slug
export PORT=3123
cd "$WORKTREE"
```

Do not skip steps. Do not reorder them. Do not substitute the commands.

### 1. Confirm the PR's code is what's checked out

Inside the worktree:

```bash
git log -1 --oneline   # should match the PR's head commit
```

The orchestrator set the worktree to the PR branch. If it doesn't match, run `git fetch origin $(git rev-parse --abbrev-ref HEAD) && git reset --hard @{u}`.

### 2. Sanity-check compilation

```bash
cargo check --workspace   # ~7-15s incremental
```

If this fails, the PR is broken. Do not continue — report the compile error in your verification comment.

### 3. Build the WASM release (required for Playwright)

Debug WASM is 253MB and will timeout Playwright. Every verification run needs release WASM.

```bash
cd "$WORKTREE/crates/kyomi-ui"
trunk build --release && gzip -9 -k dist/*_bg.wasm
cd "$WORKTREE"
```

Stable toolchain works — `src/wasm_math_shims.rs` provides the `libm` forwarders that let the linker resolve `acosh`/`asinh`/`atanh` without `build-std` + nightly.

**Never pipe `trunk build` through `tail` or truncate its output** — post-processing happens after compilation. Interruption leaves `dist/` with a 2.8KB `index.html` and no WASM. Let it finish.

### 4. Build the dev-server binary

Each worktree has its own `target/`, so the first build in a fresh worktree is a cold build (~15-20 min). Subsequent builds in the same worktree are incremental (~30s).

```bash
cd "$WORKTREE" && cargo build --profile dev-server -p kyomi-server
```

This is always required for worktree verification — the binary reads `dist/` from disk relative to its working directory, so you need the worktree's own binary pointing at the worktree's own `dist/`.

### 5. Start the dev server on the worktree's port

```bash
kill $(lsof -ti:$PORT) 2>/dev/null
cd "$WORKTREE" && set -a; source .env; set +a
PORT=$PORT FRONTEND_URL=http://localhost:$PORT "$WORKTREE/target/dev-server/kyomi" \
  > /tmp/kyomi-wt-$PORT.log 2>&1 &
```

**Critical:**
- `source .env` to get `DATABASE_URL` — triggers SaaS mode (Postgres + Redis, same shared DB as `:3000`)
- `FRONTEND_URL=http://localhost:$PORT` — for worktree-local testing Playwright hits the server directly over HTTP, so cookies don't need the `Secure` flag. The "never use `localhost`" rule applies to the nginx-proxied HTTPS path on `:3000`, **not** here.
- **Never** start the worktree server on `:3000` — that's reserved for the global `dev.kyomi.ai` instance

Wait for it to respond:

```bash
until curl -s -o /dev/null -w "%{http_code}" http://localhost:$PORT/login | grep -q 200; do sleep 2; done
```

### 6. Seed test users

```bash
python3 /home/jason/repos/kyomi/scripts/e2e-regression/seed-test-user.py
```

Idempotent — safe to run every time. Creates `e2e-test@kyomi.dev` / `E2eTestPass123!` and `e2e-admin@kyomi.dev` / `E2eAdminPass123!`. All worktrees share the same Postgres, so this seeds once for all.

### 7. Provision test data the PR needs

The verifier creates whatever data the feature needs. "No dashboards exist" is never a reason to skip testing — create one.

**Sample datasource** (for catalog, SQL editor, chart features):

```bash
# Login
curl -s -c /tmp/e2e-cookies-$PORT.txt -X POST http://localhost:$PORT/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"e2e-test@kyomi.dev","password":"E2eTestPass123!"}'

# Provision (201 = created, 409 = already exists, both are fine)
curl -s -b /tmp/e2e-cookies-$PORT.txt -X POST http://localhost:$PORT/api/v1/datasources/sample

# Wait ~5s for catalog index before testing catalog-dependent features
```

Creates "Acme Analytics (Sample)" ClickHouse datasource with 4 tables.

**Other common data needs:**
- **Dashboards**: create via Playwright flow on `/dashboards` or `POST /api/v1/dashboards` (see route handler for request shape)
- **Watches / alerts**: create via `POST /api/v1/watches` with the test user's cookies. To trigger an alert, either wait for the cron or call the manual run endpoint
- **Chat sessions**: create by sending a message — `POST /api/v1/chat/sessions` then `POST /api/v1/chat/sessions/{id}/messages`
- **Knowledge docs**: `POST /api/v1/knowledge` with markdown body

When in doubt, grep the repo for the route that creates the resource and look at its request struct.

### 8. Run Playwright

Use `.cjs` extension (repo is `"type": "module"`). Set `NODE_PATH=/home/jason/repos/kyomi/node_modules`. Full page screenshots at 1920x1080.

**Parameterize the base URL on `$PORT`** — never hardcode `localhost:3000` in verifier scripts.

```javascript
const { chromium } = require('playwright');
const PORT = process.env.PORT || '3000';

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
const page = await ctx.newPage();

await page.goto(`http://localhost:${PORT}/login`);
await page.waitForTimeout(3000);  // release WASM loads in 2-3s; debug needs 8-15s
await page.fill('input[type="email"]', 'e2e-test@kyomi.dev');
await page.fill('input[type="password"]', 'E2eTestPass123!');
await page.click('button[type="submit"]');
await page.waitForURL(url => !url.toString().includes('/login'), { timeout: 15000 });
await page.waitForTimeout(2000);
await page.screenshot({ path: `/tmp/wt-${PORT}-post-login.png`, fullPage: true });
// Now authenticated — navigate wherever you need and screenshot at each step
```

Run with the port in the environment:

```bash
PORT=$PORT NODE_PATH=/home/jason/repos/kyomi/node_modules node /tmp/verify.cjs
```

For page-level verification, prefer `/kyomi-test` which already encapsulates this flow.

### 9. Read the evidence

**Screenshots are the verification artifact.** Take at least one screenshot per page the PR affects, at 1920x1080 fullPage. A screenshot saved but not read is not evidence — open every screenshot with the Read tool and interpret it against the acceptance criterion. Paste the screenshot paths into your verification report so the evidence is auditable.

### 10. Tear down on completion

When verification is done (pass or fail):

```bash
kill $(lsof -ti:$PORT) 2>/dev/null   # stop the worktree server
# Orchestrator handles `git worktree remove` after PR merge
```

---

---

## Architecture: Three Build Artifacts

The Leptos frontend has **three separate build artifacts** that must all be current for changes to be visible:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Tailwind CSS         → crates/kyomi-ui/style/output.css │
│ 2. WASM (client-side)   → crates/kyomi-ui/dist/*.wasm      │
│ 3. Server binary         → target/{profile}/kyomi           │
└─────────────────────────────────────────────────────────────┘
```

**The server binary has TWO profiles that behave differently:**

| Profile | Binary location | Frontend loading | When to use |
|---------|----------------|-----------------|-------------|
| `dev-server` | `target/dev-server/kyomi` | Reads `dist/` **from disk at runtime** | Development (fast iteration) |
| `release` | `target/release/kyomi` | **Embeds** `dist/` at compile time | Production deploys |

**`dev-server` is the correct profile for development.** It reads `dist/` from disk, so frontend changes (WASM, CSS) take effect immediately on browser refresh — no server restart needed.

---

## Development Workflow (use this)

### One-time setup: Build the dev-server binary

```bash
cargo build --profile dev-server -p kyomi-server   # ~5-8 min, only do once
```

This binary reads `crates/kyomi-ui/dist/` from disk at runtime. You only rebuild it when server-side Rust changes (routes, server functions, database code).

### Start the dev server (SaaS mode for dev.kyomi.ai)

```bash
kill $(lsof -ti:3000) 2>/dev/null
cd /home/jason/repos/kyomi && set -a; source .env; set +a
PORT=3000 FRONTEND_URL=https://dev.kyomi.ai target/dev-server/kyomi &
```

**CRITICAL rules:**
- Source `.env` to get `DATABASE_URL` — triggers SaaS mode (Postgres + Redis)
- `FRONTEND_URL=https://dev.kyomi.ai` — cookies need the Secure flag (HTTPS access)
- **NEVER use `SELF_HOSTED=true`** — forces standalone/SQLite mode
- **NEVER use `FRONTEND_URL=http://localhost:3000`** — breaks cookies over HTTPS

### Per-change rebuild: depends on what changed

#### CSS or frontend Rust change — ~1 min incremental

```bash
cd crates/kyomi-ui
trunk build -v
# Refresh browser. Done.
```

This covers: `main.css` changes, Tailwind class changes in `.rs` files, any Rust source in `crates/kyomi-ui/src/`, and path dependency changes (chartml, kode, etc).

No server restart needed. The debug server binary reads `dist/` from disk.

Trunk runs tailwindcss as a pre-build hook. Do NOT run tailwindcss manually — it creates a hash mismatch with `index.html` and the page loads unstyled.

**CRITICAL: Never pipe trunk build to `tail` or truncate output.** Trunk's post-processing (wasm-bindgen, file copy from `.stage/` to `dist/`) happens after compilation. If the process is interrupted during this stage, `dist/` will contain only `index.html` with no WASM file (the 2.8KB problem). Always let trunk build run to full completion.

**CRITICAL: Never use `--release` for development builds.** Debug WASM builds are fast (~1 min incremental). Release builds take 5+ minutes. Use `scripts/dev/rebuild-leptos.sh --release` only for production deploys.

#### Server-side Rust change (routes, server functions, DB)

```bash
bash scripts/dev/rebuild-leptos.sh --skip-trunk
```

Or manually:
```bash
cargo build -p kyomi-server
kill $(lsof -ti:3000) 2>/dev/null
cd /home/jason/repos/kyomi && set -a; source .env; set +a
PORT=3000 FRONTEND_URL=https://dev.kyomi.ai target/debug/kyomi &
```

Only time you need to restart the server.

#### Both frontend + backend changed — do both

```bash
# Frontend first (so server doesn't embed stale WASM if you accidentally use release)
cd crates/kyomi-ui
trunk build --release && gzip -9 -k dist/*_bg.wasm

# Then server
cd /home/jason/repos/kyomi
cargo build --profile dev-server -p kyomi-server
kill $(lsof -ti:3000) 2>/dev/null
set -a; source .env; set +a
PORT=3000 FRONTEND_URL=https://dev.kyomi.ai target/dev-server/kyomi &
```

---

## Production Deploy Workflow (release builds)

Only for actual deploys to app.kyomi.ai or publishing. Not for development.

```bash
cd crates/kyomi-ui
trunk build --release && gzip -9 -k dist/*_bg.wasm
cd /home/jason/repos/kyomi && cargo build --release -p kyomi-server
```

Trunk runs tailwindcss as a pre-build hook automatically. Release profile embeds `dist/` into the binary at compile time. The binary is self-contained and portable, but every frontend change requires a full rebuild.

---

## Quick Reference: What to rebuild

| What changed | Trunk build | Server restart |
|-------------|------------|----------------|
| `main.css` only | Yes (runs tailwindcss automatically) | No |
| `.rs` in `crates/kyomi-ui/src/` | Yes | No |
| Path dep (chartml, etc.) | Yes | No |
| Server-side `.rs` (routes, DB) | No | Yes (rebuild + restart) |
| Both frontend + server | Yes | Yes |

**"No" means literally do nothing for that step.** Don't rebuild "just in case."

---

## Development Servers

| Port | What | Serves | When to use |
|------|------|--------|-------------|
| **3000** | dev-server binary | Leptos frontend (from disk) + API | **dev.kyomi.ai testing** |
| **8002** | Release/debug binary | React or Leptos + API | React reference comparison |
| **8080** | `trunk serve` | Leptos frontend (auto-rebuild on save) | Fast UI iteration (no server fns) |

### dev.kyomi.ai (port 3000) — Primary testing

- nginx on NAS (192.168.1.100) proxies `dev.kyomi.ai` → `192.168.1.200:3000`
- Runs in **SaaS mode** (Postgres + Redis)
- Use `dev-server` profile — reads `dist/` from disk, no restart for frontend changes

### trunk serve (port 8080) — Fastest iteration

```bash
trunk serve --port 8080 --address 0.0.0.0 --proxy-backend=http://localhost:3000/api/
```

- Auto-rebuilds WASM on `.rs` file changes (~27s incremental debug builds)
- **DOES NOT proxy `/leptos-api/`** — server functions won't work
- Debug WASM is ~253MB — takes 8-15 seconds to load
- Good for CSS/layout iteration only

---

## Testing with Playwright

### Login Flow

```javascript
const { chromium } = require('playwright');
const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
const page = await ctx.newPage();

await page.goto('http://localhost:3000/login');
await page.waitForTimeout(8000);  // WASM needs time to load
await page.fill('input[type="email"]', 'e2e-test@kyomi.dev');
await page.fill('input[type="password"]', 'E2eTestPass123!');
await page.click('button[type="submit"]');
await page.waitForURL(url => !url.toString().includes('/login'), { timeout: 30000 });
await page.waitForTimeout(5000);
```

### Test Users

| Email | Password | Role |
|-------|----------|------|
| `e2e-test@kyomi.dev` | `E2eTestPass123!` | Regular user (workspace admin) |
| `e2e-admin@kyomi.dev` | `E2eAdminPass123!` | Admin user (workspace owner) |

### Key Gotchas

1. **Debug WASM is huge (253MB).** Wait 8-15 seconds after navigation. Blank page = still loading.
2. **Use `.cjs` extension** for Playwright scripts (repo has `"type": "module"`).
3. **Set `NODE_PATH=/home/jason/repos/kyomi/node_modules`** to find Playwright.
4. **Screenshots must be 1920x1080.** Full page, never element-level.
5. **Root font-size is 15px** (not 16px). Rem values: `text-xl` = 18.75px, `text-2xl` = 22.5px.
6. **Use Playwright for testing, not the browse tool.** The browse tool's `type` command doesn't work reliably with Leptos inputs. `/kyomi-test` skill has the proven login flow.

---

## Common Mistakes

### "I changed a component but the page looks the same"

You're running the `release` binary which embeds WASM at compile time. Switch to `dev-server` profile which reads from disk. Or if you must use release, run the full chain: trunk → cargo → restart.

### "I changed chartml CSS but charts look the same"

Chartml is a path dependency compiled into the WASM. You need `trunk build` to recompile it. But you do NOT need a server restart if using `dev-server` profile.

### "trunk build fails with 'undefined symbol: acosh / asinh / atanh' on stable"

These symbols come from DataFusion's default math UDF set (`AcoshFunc` etc.) which references Rust's `f{32,64}::{acosh,asinh,atanh}`. The `wasm32-unknown-unknown` target ships no `libm`, so the linker can't resolve them. The fix is already in the repo: `crates/kyomi-ui/src/wasm_math_shims.rs` defines `#[unsafe(no_mangle)]` forwarders that call `libm::{acosh,asinh,...}`. If you see this error, make sure the shim file exists and that `kyomi-ui/Cargo.toml` has `libm = "0.2"` under `[target.'cfg(target_arch = "wasm32")'.dependencies]`.

### "trunk build fails with 'the option Z is only accepted on the nightly compiler'"

Outdated — trunk builds work with the stable toolchain thanks to the WASM math shim. If you see this, check that `.cargo/config.toml` doesn't have stale `build-std` flags.

### "The login works on :3000 but not on :8080"

Trunk serve only proxies `/api/`. Leptos server functions use `/leptos-api/` which isn't proxied. Test on :3000 instead, or add `--proxy-backend=http://localhost:3000/leptos-api/`.

### "cargo check passes but the UI is wrong"

`cargo check` only validates Rust compilation. It doesn't rebuild Tailwind, WASM, or verify CSS classes exist. For visual verification: trunk build + refresh (dev-server) or trunk → cargo → restart (release).

### "Page loads unstyled (raw HTML, no CSS)"

**Root cause:** You ran `tailwindcss` manually AFTER `trunk build`. Trunk's pre-build hook runs tailwindcss and generates `index.html` referencing the CSS file hash at that time. Running tailwindcss again afterward replaces `output.css` with a different hash, but `index.html` still references the old hash. The browser requests the old hash, gets a 404, and the page is unstyled.

**Fix:** Never run tailwindcss separately before trunk. Just run `trunk build` — it runs tailwindcss as a pre-build hook automatically (configured in `Trunk.toml`). If you need to regenerate CSS, run trunk build again.

### "Select dropdown doesn't appear in Playwright"

Native `<select>` popups are rendered by the OS, not the DOM. Only custom `StyledSelect` dropdowns are testable in headless Playwright.

---

## Verification Checklist

Before declaring any UI change "done":

- [ ] `cargo check --workspace` passes
- [ ] Tailwind CSS rebuilt
- [ ] Trunk WASM rebuilt (if .rs files changed)
- [ ] Server restarted (only if server-side code changed)
- [ ] Browser refreshed (hard refresh: Ctrl+Shift+R)
- [ ] Playwright test captures screenshots of affected pages
- [ ] Screenshots reviewed by evaluator agent (separate from test writer)
