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
│ 3. Server binary         → target/release/kyomi             │
│    (embeds the WASM from dist/ at compile time)             │
└─────────────────────────────────────────────────────────────┘
```

**If you change a `.rs` file in `crates/kyomi-ui/`:**
- You MUST rebuild the WASM (trunk build)
- You MUST rebuild the server binary (cargo build) AFTER trunk finishes
- Just running `cargo build` alone does NOT rebuild the WASM

**If you change `style/main.css` only:**
- Run `tailwindcss` to regenerate `output.css`
- Then trunk build → cargo build (same chain)

**If you change server-side Rust only (routes, server functions):**
- `cargo build` alone is sufficient — no trunk needed

## The Build Chain (in order)

```bash
# Step 1: Rebuild Tailwind CSS (fast, ~200ms)
cd crates/kyomi-ui
tailwindcss --input style/main.css --output style/output.css --content "src/**/*.rs"

# Step 2: Rebuild WASM via trunk (slow, ~2-5 min release)
# MUST use nightly — build-std requires it (configured in .cargo/config.toml)
RUSTUP_TOOLCHAIN=nightly trunk build --release
gzip -9 -k dist/*_bg.wasm

# Step 3: Rebuild server binary (slow, ~5-16 min release)
cd /home/jason/repos/kyomi
cargo build --release -p kyomi-server

# Step 4: Restart the server
kill $(lsof -ti:3000) 2>/dev/null
SELF_HOSTED=true PORT=3000 FRONTEND_URL=http://localhost:3000 \
  DATA_DIR=/home/jason/repos/kyomi/data \
  /home/jason/repos/kyomi/target/release/kyomi &
```

**CRITICAL: Step 3 embeds whatever is in `dist/` at compile time.** If you run step 3 before step 2 finishes, the server binary will embed the OLD WASM.

### Shortcut: Use `scripts/dev/trunk-build.sh`

```bash
bash scripts/dev/trunk-build.sh --release
```

This builds trunk to a staging dir and swaps atomically, preventing the live server from serving half-written files.

---

## Development Servers

| Port | What | Serves | When to use |
|------|------|--------|-------------|
| **3000** | Standalone binary | Leptos frontend (from embedded dist/) + API | Production-like testing |
| **8002** | Docker API container | React frontend + API | React reference comparison |
| **8080** | `trunk serve` | Leptos frontend (auto-rebuild) + proxied API | Fast iteration on UI |

### Port 8080 (trunk serve) — Fast Iteration

```bash
trunk serve --port 8080 --address 0.0.0.0 --proxy-backend=http://localhost:3000/api/
```

- Auto-rebuilds WASM on `.rs` file changes (~27s incremental)
- Proxies `/api/` to the standalone binary on :3000
- **DOES NOT proxy `/leptos-api/`** — Leptos server functions won't work unless you add another proxy flag
- Debug WASM is ~253MB — takes 8-15 seconds to load in the browser
- Good for CSS/layout iteration, NOT for testing server functions

### Port 3000 (standalone binary) — Full Testing

- Serves embedded Leptos frontend + all API routes + Leptos server functions
- **Requires full rebuild chain** (trunk → cargo) for .rs changes to take effect
- In debug profile: reads from `dist/` on disk (no server rebuild needed for frontend)
- In release profile: WASM is compiled into the binary (rebuild required)

---

## Testing with Playwright

### Login Flow

```javascript
const { chromium } = require('playwright');
const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
const page = await ctx.newPage();

await page.goto('http://localhost:3000/login');
await page.waitForTimeout(8000);  // Debug WASM needs 8-15s to load
await page.fill('input[type="email"]', 'e2e-test@kyomi.dev');
await page.fill('input[type="password"]', 'E2eTestPass123!');
await page.click('button[type="submit"]');
await page.waitForURL(url => !url.toString().includes('/login'), { timeout: 30000 });
await page.waitForTimeout(5000);  // Wait for post-login hydration
```

### Test Users

| Email | Password | Role |
|-------|----------|------|
| `e2e-test@kyomi.dev` | `E2eTestPass123!` | Regular user (workspace admin) |
| `e2e-admin@kyomi.dev` | `E2eAdminPass123!` | Admin user (workspace owner) |

### Key Gotchas

1. **Debug WASM is huge (253MB).** Wait 8-15 seconds after navigation before asserting anything. If a page looks blank, increase the wait — don't assume it's broken.

2. **Use `.cjs` file extension** for Playwright scripts. The repo has `"type": "module"` in package.json, but test scripts use CommonJS.

3. **Set `NODE_PATH`** to find Playwright: `NODE_PATH=/home/jason/repos/kyomi/node_modules`

4. **Screenshots must be 1920x1080.** Never use element-level screenshots or clip options. The evaluator needs full page context.

5. **Root font-size is 15px** (not 16px). Tailwind rem values compute differently:
   - `text-sm` (0.875rem) = 13.125px
   - `text-base` (1rem) = 15px
   - `text-lg` (1.125rem) = 16.875px
   - `text-xl` (1.25rem) = 18.75px
   - `text-2xl` (1.5rem) = 22.5px

---

## Common Mistakes (learn from our pain)

### "I changed a component but the page looks the same"

**Root cause:** You tested against a stale binary. The standalone binary on :3000 embeds the WASM at compile time.

**Fix:** Run the full build chain: tailwind → trunk build → cargo build → restart server.

### "The login works on :3000 but not on :8080"

**Root cause:** Trunk serve only proxies `/api/` to :3000. Leptos server functions use `/leptos-api/` which isn't proxied.

**Fix:** Either:
- Test on :3000 (full stack)
- Add `--proxy-backend=http://localhost:3000/leptos-api/` to the trunk serve command
- Login via `fetch('/api/v1/auth/login', ...)` in the browser console (sets cookies, bypasses Leptos login form)

### "The toast doesn't animate"

**Root cause (historical):** The code used `animate-in slide-in-from-right` classes from the `tailwindcss-animate` plugin, but that plugin isn't installed in the Leptos Tailwind CLI build. The classes silently did nothing.

**Fix (applied):** Custom keyframes and utility classes defined in `main.css`. Use `animate-slide-in-right` (our custom class), not `animate-in slide-in-from-right` (tailwindcss-animate plugin).

### "trunk build fails with 'the option Z is only accepted on the nightly compiler'"

**Root cause:** The `.cargo/config.toml` enables `build-std` with `-Zpanic=immediate-abort` for WASM targets. This requires the nightly toolchain.

**Fix:** Always prefix with `RUSTUP_TOOLCHAIN=nightly`:
```bash
RUSTUP_TOOLCHAIN=nightly trunk build --release
```

### "Two trunk builds are running at the same time"

**Root cause:** You launched trunk build twice (e.g., background + foreground). They both try to write to `dist/` and can corrupt each other.

**Fix:** Check `ps aux | grep "trunk build"` before launching. Kill duplicates with `kill <pid>`.

### "cargo check passes but the UI is wrong"

**`cargo check` only validates Rust compilation.** It does NOT:
- Rebuild Tailwind CSS
- Rebuild WASM
- Verify CSS classes exist in output.css
- Verify the server binary is current

For full verification: trunk build + cargo build + restart + Playwright test.

### "The browse tool can't authenticate on :8080"

**Root cause:** The browse tool's `type` command appends text to inputs (doesn't clear first). Leptos inputs may not react to programmatic `type` events properly.

**Fix:** Use Playwright for testing, not the browse tool. The `/kyomi-test` skill has the proven login flow with `page.fill()` which clears and types in one step.

---

### "Dashboard cards have 0px border-radius" (false positive)

**Root cause:** The Playwright selector matched non-card elements (icons, buttons) that happened to be inside the dashboard grid. The actual card containers have proper `rounded-lg`.

**Fix:** Use specific selectors. Dashboard cards have class `group relative`. Don't match generic elements and assume they're cards.

### "Select dropdown doesn't appear in Playwright"

**Root cause:** If the page uses a native `<select>` element (not the custom `StyledSelect` component), Playwright can't capture its dropdown — native select popups are rendered by the OS/browser chrome, not the DOM.

**Fix:** Only test custom `StyledSelect` dropdowns. Look for elements with `role="listbox"` or the `CONTENT_CLASS` from `select.rs`. Native selects won't show animation classes.

---

## Verification Checklist

Before declaring any UI change "done":

- [ ] `cargo check --workspace` passes
- [ ] Tailwind CSS rebuilt (`tailwindcss --input ... --output ...`)
- [ ] Trunk WASM rebuilt (`trunk build` or `trunk build --release`)
- [ ] Server binary rebuilt AFTER trunk (`cargo build --release -p kyomi-server`)
- [ ] Server restarted with new binary
- [ ] Playwright test captures screenshots of affected pages
- [ ] Screenshots reviewed by evaluator agent (separate from test writer)
- [ ] Dark mode checked (if applicable)
- [ ] Mobile viewport checked (if applicable)
