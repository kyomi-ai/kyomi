# Build & Testing Guide

**The #1 source of wasted time: testing against a stale binary.**

This document exists because we keep making the same mistakes. Read it before verifying any UI change.

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

### "trunk build fails with 'the option Z is only accepted on the nightly compiler'"

This error is outdated — trunk builds now work with the stable toolchain. If you see this, check that `.cargo/config.toml` doesn't have stale `build-std` flags.

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
