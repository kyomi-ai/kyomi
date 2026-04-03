# WASM Binary Size & Time-to-Interactive Optimization

**Date:** 2026-04-03
**Status:** Design
**Goal:** Reduce initial page load from 30s black screen to <3s visible content on slow connections

## Problem

The Leptos frontend compiles to a 32MB WASM binary (release, wasm-opt). On a slow connection (1 Mbps), this takes ~30 seconds to download. During that time the user sees a black screen — no loading indicator, no content, nothing. The HTML shell is empty and forces `class="dark"`, producing a dark void.

### Current State

| Metric | Value |
|---|---|
| WASM binary (release) | 32 MB |
| WASM gzipped (on wire, nginx) | ~8-10 MB |
| Time to interactive (1 Mbps) | ~30s |
| Time to first paint | ~30s (nothing visible until WASM mounts) |
| Routes in app | 25+ |
| All routes in single WASM | Yes (monolithic) |
| Caching | `no-cache` with ETag (revalidates every load) |

### Size Breakdown (Pre-LTO .rlib)

| Category | Pre-LTO Size | % |
|---|---|---|
| Kyomi app code | 185 MB | 26% |
| DataFusion + sqlparser | 148 MB | 21% |
| Other (serde, regex, chrono-tz, etc) | 196 MB | 28% |
| Leptos framework | 55 MB | 8% |
| Arrow (columnar data) | 50 MB | 7% |
| Browser APIs (web-sys, js-sys) | 39 MB | 6% |
| Kode editor + tree-sitter | 17 MB | 2% |
| ChartML | 11 MB | 2% |
| **Total pre-LTO** | **703 MB** | |
| **Post-LTO + wasm-opt** | **32 MB** | |

LTO eliminates 95% of dead code. DataFusion + Arrow + their transitive deps (chrono-tz, tokio, simba, nalgebra) account for ~200 MB pre-LTO.

## Design

Five layers of optimization, ordered by phase. Each layer is independent — later phases build on earlier ones but don't require them.

### Phase 1: Loading Shell & Immutable Caching

**Goal:** Immediate visual feedback + never re-download unchanged WASM.

#### 1a. Loading Shell in `index.html`

Add a visible loading screen directly in the HTML template (`crates/kyomi-ui/index.html`). This renders before any JS or WASM loads.

**Requirements:**
- Centered Kyomi animated logo (already exists as an SVG asset)
- Pure CSS animation (no JS dependency)
- System theme detection via inline `<script>` before CSS loads (removes forced-dark-on-light problem)
- Disappears when `mount_to_body(App)` replaces the body content

**Theme detection script (inline in `<head>`):**
```javascript
(function() {
  var d = document.documentElement;
  try {
    var stored = localStorage.getItem('theme-preference');
    if (stored === 'dark' || (!stored && matchMedia('(prefers-color-scheme:dark)').matches)) {
      d.classList.add('dark');
    } else {
      d.classList.remove('dark');
    }
  } catch(e) { /* localStorage unavailable */ }
})();
```

This replaces the current hardcoded `class="dark"` on `<html>`.

#### 1b. Immutable Caching for Content-Hashed Files

**Already coded** in `apps/server/src/leptos_frontend.rs`. The `is_content_hashed()` function detects trunk-generated filenames (e.g. `kyomi-ui-<hash>_bg.wasm`) and serves them with `Cache-Control: public, max-age=31536000, immutable`.

After the first visit, the WASM is cached forever. The filename hash changes on each build, so stale content is never served.

**Requires:** Server binary rebuild + restart.

#### 1c. Pre-Compression at Build Time

Add a post-build step to gzip (or brotli) the WASM:

```bash
gzip -9 -k dist/*.wasm
# or: brotli -9 dist/*.wasm
```

The Rust server can serve the pre-compressed file when the client sends `Accept-Encoding: gzip`. Alternatively, nginx's `gzip_static on` (already enabled) serves `.gz` files automatically for static serving.

**Expected impact on wire size:** 32 MB → ~8 MB (gzip) or ~6.5 MB (brotli).

### Phase 2: Build Optimization — `build-std` with `panic_immediate_abort`

**Goal:** Eliminate dead standard library code and panic formatting infrastructure.

The pre-compiled standard library includes panic formatting, backtrace support, and string formatting machinery — none of which is useful in WASM (panics trap immediately in the browser). Rebuilding std from source with `panic_immediate_abort` strips all of this.

**Setup:**

1. Install nightly and pin version:

```toml
# rust-toolchain.toml (project root)
[toolchain]
channel = "nightly-2026-04-01"
targets = ["wasm32-unknown-unknown"]
```

2. Create `.cargo/config.toml`:

```toml
[target.wasm32-unknown-unknown]
rustflags = ["--cfg=has_std"]

[unstable]
build-std = ["std", "panic_abort", "core", "alloc"]
build-std-features = ["panic_immediate_abort"]
```

3. Build with nightly:

```bash
RUSTUP_TOOLCHAIN=nightly trunk build --release
# or if using cargo-leptos:
cargo +nightly leptos build --release
```

**Note:** `build-std` only applies to the wasm32 target. The `[target.wasm32-unknown-unknown]` section ensures it doesn't affect server builds. The `has_std` cfg flag prevents compilation errors in crates that check for std availability.

**Expected savings:** 5-15% of binary size (1.5-5 MB).

**Risk:** Nightly features can break between releases. The pinned version in `rust-toolchain.toml` mitigates this. Only the WASM build uses nightly — the server binary continues on stable.

### Phase 3: Code Splitting with `#[lazy_route]` — DEFERRED

**Status:** Deferred. Research (2026-04-03) found that `#[lazy_route]` code splitting requires `cargo-leptos` which only supports SSR+hydration, not CSR-only builds. Our app uses trunk with CSR (`mount_to_body`). Adopting code splitting requires first migrating to SSR — a larger architectural change.

**Revisit when:** SSR is adopted for other reasons, or if `wasm_split_cli` gains standalone CSR support.

**Original goal:** Only download code for the current page. Lazy-load other pages (and their heavy dependencies) on navigation.

#### How `#[lazy_route]` Works

Leptos 0.8 provides the `#[lazy_route]` macro. At build time, `cargo leptos build --split` compiles the app, then uses `wasm-bindgen --split` to separate the binary into:

- **Core chunk:** Shared framework code (Leptos, router, layout, common components)
- **Page chunks:** Per-route code that's loaded on demand via dynamic `import()`

All chunks share the same WASM linear memory. Rust code in a lazy chunk calls Rust code in the core chunk directly — no JS bridge, no serialization, no data copying. DataFusion transforms execute in the same address space as the chart renderers.

#### Splitting Strategy

**Core (loads immediately):**
- Leptos framework + router
- Layout component (sidebar, navigation)
- Auth pages (login, signup, recover) — lightweight, needed first
- Theme provider, navigation progress
- Common components (Spinner, Alert, Skeleton)

**Lazy chunks (load on navigation):**

| Chunk | Routes | Heavy Dependencies |
|---|---|---|
| **Dashboard viewer** | `/dashboard/:id` | chartml-*, chartml-datafusion (DataFusion), markdown rendering |
| **Dashboard editor** | `/dashboard/:id/edit` | kode-leptos (WYSIWYG), chartml-*, chartml-datafusion |
| **Chat** | `/chat`, `/chat/:id` | Streaming markdown, chartml-*, chartml-datafusion |
| **SQL Editor** | `/sql-editor` | kode-leptos (code editor), Arrow IPC |
| **Settings** | `/settings/*` (8 sub-routes) | Forms, datasource config |
| **Knowledge** | `/knowledge` | Knowledge graph UI |
| **Watches** | `/watches` | Watch list |
| **Home** | `/` | Dashboard list |

**Critical boundary rule:** Chart types (`ChartML`, `DataFusionTransform`, `ChartElement`) must NOT appear in the core chunk. They must only be referenced inside lazy route components. If a chart type leaks into the layout or router, the linker pulls DataFusion into the core.

This means:
- The `Layout` component cannot import or reference chart types
- The router definition uses `LazyRoute` trait objects, not concrete component types
- Any shared chart configuration (palette, renderers) lives inside the lazy components

#### Build Tooling Migration: trunk → cargo-leptos

Code splitting requires `cargo leptos build --split`. Trunk does not support WASM splitting.

**Migration steps:**

1. Install cargo-leptos: `cargo install cargo-leptos`

2. Add `Cargo.toml` metadata:

```toml
[package.metadata.leptos]
# Output directory
site-root = "target/site"
site-pkg-dir = "pkg"

# WASM build settings
lib-profile-release = "wasm-release"
bin-profile-release = "release"

# Tailwind integration
tailwind-input-file = "crates/kyomi-ui/style/main.css"
tailwind-config-file = "crates/kyomi-ui/tailwind.config.js"

# Assets
assets-dir = "crates/kyomi-ui/public"
```

3. Restructure the crate to have both `lib.rs` (WASM) and `bin` (server) entry points, or keep them as separate crates with cargo-leptos workspace configuration.

4. Replace `trunk build --release` with `cargo leptos build --release --split`.

5. Update the server to serve from `target/site/pkg/` instead of `crates/kyomi-ui/dist/`.

6. Update the dev workflow docs and memory.

**Risk:** This is the highest-effort change. cargo-leptos has a different dev server, different hot-reload mechanism, and different output structure. Need to verify:
- Tailwind pre-build hook works
- Dev-server disk-reading workflow still functions
- All 100+ server functions register correctly
- The split build produces correct chunk boundaries

**Recommendation:** Prototype on a branch. Convert one route (e.g. Settings) to `#[lazy_route]`, run `cargo leptos build --split`, and verify chunk sizes and runtime behavior before migrating all routes.

#### Expected Impact

| Scenario | Core Chunk | Lazy Chunks | Total on Wire (gzipped) |
|---|---|---|---|
| Login page | ~5-8 MB | 0 | ~1.5-2.5 MB |
| Dashboard view | ~5-8 MB + dashboard chunk | ~3-5 MB | ~2.5-4 MB |
| SQL Editor | ~5-8 MB + editor chunk | ~2-4 MB | ~2-3.5 MB |
| All pages visited | ~5-8 MB + all chunks | = ~32 MB total | ~8-10 MB |

Time to interactive on 1 Mbps: **~30s → ~8-12s** (core only), with additional chunks loading transparently during navigation.

### Phase 4: Serialization Optimization

**Goal:** Reduce serde overhead in server function communication.

The Leptos book recommends `miniserde` or `serde-lite` as lighter alternatives to `serde` for server function serialization. Our 100+ server functions all use standard `serde`.

**Options:**
- `miniserde` — much smaller, but limited (no enums with data, no custom deserialize)
- `serde-lite` — smaller than serde, supports more types
- `bitcode` — binary encoding, very compact, fastest

**Recommendation:** Defer this. Serde adds ~30-50 KB to the binary after LTO — negligible compared to DataFusion's contribution. Only pursue if we hit diminishing returns on other optimizations.

### Phase 5: Dependency Audit

**Goal:** Remove unused or unnecessarily heavy dependencies from the WASM target.

**Candidates to investigate:**
- `chrono-tz` (18 MB pre-LTO) — full timezone database. If only UTC is needed in WASM, feature-gate it
- `nalgebra` + `simba` + `wide` (23 MB pre-LTO) — linear algebra. Used by chartml scatter regression? If so, moves to lazy chunk with code splitting
- `tokio` (6 MB pre-LTO) — async runtime pulled in by DataFusion. Moves to lazy chunk with code splitting
- `syn` (7 MB pre-LTO) — proc macro crate, should NOT be in runtime binary. Investigate why it's linked

**Recommendation:** Most of these resolve naturally with code splitting (Phase 3) — they move to lazy chunks. The `syn` inclusion is a bug worth investigating independently.

## Implementation Order

```
Phase 1a: Loading shell in index.html
Phase 1b: Deploy immutable caching (server rebuild)
Phase 1c: Pre-compress WASM at build time
   → Result: Animated Kyomi logo visible in <1s, 32 MB cached after first visit

Phase 2: build-std with panic_immediate_abort
   → Result: ~27 MB WASM (~5 MB saved)

Phase 3: Code splitting (biggest impact, most effort)
   3a: Prototype — convert one route to #[lazy_route], verify
   3b: Migrate from trunk to cargo-leptos
   3c: Apply #[lazy_route] to all page groups
   3d: Verify chunk boundaries (DataFusion not in core)
   → Result: ~5-8 MB initial load, ~1.5-2.5 MB gzipped

Phase 4: Serialization optimization (defer unless needed)
Phase 5: Dependency audit (most resolves with Phase 3)
```

## Success Metrics

| Metric | Before | After Phase 1+2 |
|---|---|---|
| First paint | 30s (black screen) | <1s (animated Kyomi logo) |
| WASM binary | 32 MB | 30 MB |
| WASM on wire (first visit) | ~10 MB (nginx on-the-fly gzip) | 8.8 MB (pre-compressed, immutable cache) |
| WASM on wire (return visit) | ~10 MB (re-downloads every time) | 0 bytes (immutable cache) |
| Perceived experience | "App is broken" | "App is loading" (branded) |
| Theme on load | Forced dark | Matches system preference |
