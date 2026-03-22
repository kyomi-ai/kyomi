# Leptos Frontend Migration — Zero-JS Architecture Design

**Status:** Research complete, ready for Phase 0
**Date:** 2026-03-20
**Author:** Jason + Claude (research collaboration)

## Executive Summary

Migrate Kyomi's frontend from React + REST API to a pure Rust/Leptos stack with **zero JavaScript dependencies**. The existing Axum backend and service layer remain unchanged. Pages migrate incrementally — React and Leptos run side by side in the same server.

### Why

| Problem (React + REST) | Solution (Leptos) |
|------------------------|-------------------|
| Two languages (JS + Rust), two build systems | Rust everywhere, `cargo leptos build` |
| Duplicated types — every API has a Rust struct AND a JS shape | Shared Rust types, compiler-enforced contract |
| Serialization overhead — Rust → JSON → network → parse → JS → React state | Server functions — typed Rust function calls, auto-serialized |
| Complex auth machinery — httpOnly cookies, 401 interceptor, refresh queue, retry | Server fns run on server — auth is just an Axum extractor |
| 5+ React Context providers managing overlapping server state | Flat signal graph — Resources replace Context + Query cache |
| Heavy JS bundle — React + Router + TanStack + Radix + Monaco + TipTap + D3 + ChartML (~1.5MB+) | ~35KB Leptos WASM + pure Rust dependencies |
| Two test ecosystems — Vitest + cargo test | `cargo test` for everything |
| Server-side chart rendering requires Node.js pod / headless Chrome | `chartml-rs` renders SVG in pure Rust — same binary, microseconds |

---

## Technology Stack

### Frontend Framework

| Component | Choice | Rationale |
|-----------|--------|-----------|
| **UI Framework** | [Leptos 0.8](https://github.com/leptos-rs/leptos) | Fine-grained signals (SolidJS-inspired), SSR + hydration, smallest WASM bundles (~35KB), first-class Axum integration, server functions |
| **Component Library** | [Singlestage UI 0.4](https://github.com/adoyle0/singlestage-ui) | shadcn/ui aesthetic, Tailwind-based, 40 components, feature-flagged, WAI-ARIA accessible |
| **CSS** | Tailwind 4 | Native `cargo-leptos` support, same design tokens as shadcn |
| **Class Merging** | [tailwind-fuse](https://github.com/gaucho-labs/tailwind-fuse) | Rust `cn()` function — equivalent to `clsx` + `tailwind-merge` |

#### Why Leptos Over Dioxus and Yew

| Criteria | Leptos | Dioxus | Yew |
|----------|--------|--------|-----|
| WASM bundle | ~25-35KB | ~45-60KB | ~110-130KB |
| Reactivity | Fine-grained signals (surgical DOM updates) | Virtual DOM + signals | Virtual DOM (Elm-style diffing) |
| SSR/Hydration | First-class, islands architecture | Supported | Afterthought, manual setup |
| Axum integration | Native (same server) | Separate | Separate |
| Server functions | Built-in, type-safe | Built-in | Not built-in |
| Cross-platform | Web only | Web + Desktop + Mobile + TUI | Web only |
| Funding | Community | YC-backed ($500K) | Community |
| Stars | 20.4K | 35.4K | 32.5K |

Dioxus would be the choice if cross-platform (web + desktop) from a single codebase were required. Kyomi's desktop app (`kyomi-desktop`) is already a separate Tauri app, so web-only Leptos with smallest bundles and best Axum integration wins.

### Replacing JavaScript Dependencies

Every JS dependency has been mapped to a pure Rust replacement. **No JavaScript required.**

#### Chart Rendering — chartml-rs (ours)

| | JS ChartML (`@chartml/core` + D3) | chartml-rs |
|---|---|---|
| Location | `~/repos/chartml-rs` | Same |
| Language | JavaScript + D3.js | Pure Rust |
| Rendering | D3 → SVG/Canvas via DOM | Rust → ChartElement IR → SVG |
| WASM | N/A (JS) | Compiles to `wasm32-unknown-unknown` |
| Server-side | Needs Node.js/headless Chrome | Pure Rust function call → SVG string |
| Size | ~10,600 LOC | Same |
| Tests | JS test suite | 203 Rust tests, all passing |
| Chart types | Bar, Line, Area, Pie, Doughnut, Scatter, Bubble, Metric | All 7 implemented |
| Architecture | D3 selections + DOM manipulation | Plugin registry → ChartElement IR → Leptos views or SVG strings |

**Note:** Named data sources (BigQuery proxy, etc.), interactive tooltips, and responsive resize are Kyomi consumer-side plugins, not core chartml features. These will be implemented as Kyomi-specific plugins for chartml-rs, not expected in the core library.

#### Client-Side SQL — Apache DataFusion (replaces DuckDB-WASM)

| | DuckDB-WASM (JS) | DataFusion (Rust) |
|---|---|---|
| Language | C++ compiled to WASM via Emscripten, JS bindings | **Pure Rust** |
| WASM target | Emscripten (`wasm32-unknown-emscripten`) | Native (`wasm32-unknown-unknown`) |
| Arrow integration | Converts to/from Arrow | **Built on Arrow natively** |
| Size | ~9.6MB gzipped | ~10MB compressed (comparable) |
| SQL coverage | Full analytical SQL | Full analytical SQL (GROUP BY, window functions, CTEs, percentiles) |
| Rust interop | Impossible (Emscripten ABI) | Direct Rust function calls |

**Why DuckDB-RS can't compile to WASM:** `duckdb-rs` wraps `libduckdb` C++ via FFI. The `wasm32-unknown-unknown` target has no C++ toolchain. GitHub issues #56 and #168 were both closed — the DuckDB team's position is "use the JS package for browsers."

**DataFusion WASM is proven:**
- [datafusion-wasm-bindings](https://github.com/datafusion-contrib/datafusion-wasm-bindings) — published to npm
- [datafusion-wasm-playground](https://github.com/datafusion-contrib/datafusion-wasm-playground) — live browser demo
- Requires disabling `compression` feature flag (removes zstd C dependency)

**SQL dialect differences (minor):**
- `PERCENTILE_CONT`: DuckDB uses `PERCENTILE_CONT(column, 0.95)`, DataFusion uses `PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY column)`
- `DATE_TRUNC`: Both support it, argument order may differ
- Standard ANSI SQL (GROUP BY, JOINs, CTEs, HAVING, window functions) is identical

**Architecture simplification:** The current DuckDB setup uses ~2,000 lines of JS for multi-tab worker coordination (`duckdb-worker.js`, `duckdb-coordinator.js`, `duckDbService.js` — dedicated Worker per tab, SharedWorker coordinator, Web Locks, heartbeat monitoring). With DataFusion in the same WASM module as Leptos, this reduces to direct Rust function calls. Web Worker isolation can be added later if needed for large queries.

#### Forecasting — quackstats-rs (ours, port from QuackStats)

| | QuackStats (DuckDB extension) | quackstats-rs (port) |
|---|---|---|
| Location | `~/repos/quackstats` | New crate (extracted from same repo) |
| Language | **Already Rust** | Same Rust code |
| Integration | DuckDB VTab (C API FFI) | Direct Rust function calls |
| License | MIT (Alytic Pty Ltd) | Same |
| Size | ~2,916 LOC total (~2,400 useful, ~530 DuckDB FFI to strip) | ~2,400 LOC |
| Models | ETS, ETS Seasonal, Linear, Exponential, Logistic, Auto (CV-based selection) | Same — all model code is DuckDB-independent |

**The port is trivial** — QuackStats is already Rust. The forecasting logic in `models.rs` (1,237 lines) operates on `TimeSeries` structs (`Vec<i32>` timestamps + `Vec<f64>` values) with no DuckDB dependency. The port is: strip DuckDB FFI layer (~530 lines), keep core algorithms, accept `Vec<f64>` instead of DuckDB table reads.

**Dependencies (all pure Rust, all WASM-compatible):**
- `augurs-ets` 0.9 — ETS model fitting
- `augurs-seasons` 0.9 — Periodogram-based seasonal detection
- `linregress` 0.5 — OLS regression
- `statrs` 0.18 — Statistical distributions (Student's t)
- `levenberg-marquardt` 0.14 — Nonlinear optimization (logistic fitting)
- `nalgebra` 0.33 — Linear algebra (Jacobians)

**Integration with DataFusion:**
```rust
// 1. Aggregate in DataFusion
let df = ctx.sql("SELECT date, SUM(revenue) FROM results GROUP BY date ORDER BY date").await?;
let batches = df.collect().await?;

// 2. Extract time series from Arrow batches
let timestamps: Vec<i32> = /* from date column */;
let values: Vec<f64> = /* from value column */;

// 3. Forecast using quackstats-rs (direct Rust call)
let ts = TimeSeries { timestamps, values };
let result = forecast_auto(&ts, horizon, confidence)?;
```

#### Code Editing — kode-leptos (built, pure Rust)

**Status: Built and in use.** The `kode-leptos` editor (`~/repos/kode/kode-leptos/`) is a pure Rust code editor for Leptos, already integrated into the dashboard editor and chart builder.

**Architecture:** Virtual-viewport rendering with hidden textarea for input capture. Uses `ropey` for the text buffer and `arborium` (tree-sitter) for syntax highlighting. Zero JavaScript.

**Building blocks (all compile to `wasm32-unknown-unknown`):**

| Crate | Purpose | WASM Status |
|-------|---------|-------------|
| `arborium` 2.16 + `arborium-theme` | Tree-sitter syntax highlighting (Tokyo Night theme) | Compiles cleanly — pure Rust |
| `ropey` 1 | Text rope buffer (efficient edits, cheap clone for undo) | Compiles cleanly — pure Rust |
| `web-sys` | Browser APIs (textarea, keyboard events, clipboard, DOM Range) | Native WASM bindings |

**Supported languages:** SQL, YAML, Markdown, Plain text.

**Features:**
- Virtual viewport rendering (only visible lines + 20-line buffer)
- Cursor tracking with pixel-perfect positioning via DOM Range API
- Full keyboard support: arrow keys, Ctrl+Z/Y undo/redo, Ctrl+A select all, Ctrl+Shift+D duplicate
- Mouse support: click, double-click (word), triple-click (line), drag selection
- IME composition support for international input
- Line numbers with dynamic gutter width

**API:**
```rust
use kode_leptos::{CodeEditor, Language};

view! {
    <CodeEditor
        language=Signal::stored(Language::Sql)
        content=content_signal
        on_change=Arc::new(move |text| { /* handle change */ })
    />
}
```

**Current consumers:** Dashboard editor (Markdown mode), Chart builder (SQL mode), SQL Editor (planned).

#### Rich Text / Dashboard Editing — kode-leptos markdown mode + comrak

Instead of TipTap/ProseMirror WYSIWYG, use kode-leptos in Markdown mode with live preview. Dashboard content is stored as plain markdown with fenced `chartml` blocks. **Status: Built and in use** — the dashboard editor already uses this approach.

**Markdown parsing (pure Rust, WASM-compatible):**
- `comrak` — GFM-compatible, full AST, fenced code block support
- `pulldown-cmark` — faster, pull-parser, CommonMark

**Chart embedding:** Fenced ` ```chartml ``` ` blocks are intercepted during rendering and replaced with chartml-rs chart components.

**Trade-off:** Not true WYSIWYG — users see markdown syntax. But Obsidian proved this model works and users accept it.

#### Other Replacements

| JS Dependency | Rust Replacement | Notes |
|--------------|-----------------|-------|
| SimpleWebAuthn (`@simplewebauthn/browser`) | `web_sys` WebAuthn API directly | WebAuthn is a browser API — the JS lib is just a thin wrapper (~370 lines). Rewrite directly against `web_sys::PublicKeyCredential` etc. |
| Driver.js (onboarding tours) | Custom Leptos component | Simple: measure element position, render highlight overlay, show tooltip. Not a complex library. |
| Apache Arrow JS | `arrow-rs` crate | Production-ready, zero-copy, WASM-compatible. Arguably *better* than the JS implementation. |
| ag-Grid | **Already unused** — not imported anywhere in frontend. Remove from package.json. |
| html2canvas | **Already unused** — not imported anywhere in frontend. Remove from package.json. |
| Apache Arrow JS | **Already unused** — not imported anywhere in frontend. Remove from package.json. |

---

## Architecture Comparison

### Current: React + REST API

```
Browser                                         Server
┌──────────────────────────────┐    ┌──────────────────────────────┐
│  React SPA (Vite bundle)     │    │  Axum                        │
│  ├── React Router            │    │  ├── REST Routes (48 modules)│
│  ├── TanStack Query          │    │  ├── WebSocket Handlers      │
│  ├── Axios apiClient         │    │  ├── Service Layer           │
│  ├── 5+ Context Providers    │    │  │   ├── kyomi-auth          │
│  ├── 30+ Pages, 122+ Comps   │    │  │   ├── kyomi-agent         │
│  ├── ChartML (JS + D3)       │    │  │   └── kyomi-*             │
│  ├── Monaco Editor           │    │  ├── DbPool (PG/SQLite)      │
│  ├── TipTap/ProseMirror      │    │  ├── KV Store (Redis/Mem)    │
│  └── DuckDB WASM (workers)   │    │  └── rust-embed (serves SPA) │
│         │                    │    │                              │
│         │ HTTP JSON + WS     │    │                              │
└─────────┼────────────────────┘    └──────────────────────────────┘
          └──── Network ───────────────────┘
```

### Target: Leptos Full-Stack (Zero JS)

```
Browser                                         Server
┌──────────────────────────────┐    ┌──────────────────────────────┐
│  Leptos (hydrated WASM ~35KB)│    │  Axum + Leptos               │
│  ├── Signals (fine-grained)  │    │  ├── SSR Renderer            │
│  ├── Leptos Router           │    │  ├── Server Functions (typed) │
│  ├── Singlestage UI (shadcn) │    │  ├── WebSocket Handlers      │
│  ├── chartml-rs (SVG)        │    │  ├── Service Layer           │
│  ├── kode-leptos (arborium)  │    │  │   ├── kyomi-auth          │
│  ├── DataFusion (Arrow SQL)  │    │  │   ├── kyomi-agent         │
│  └── quackstats-rs (forecast)│    │  │   └── kyomi-*             │
│         │                    │    │  ├── DbPool (PG/SQLite)      │
│         │ Server Functions   │    │  ├── KV Store (Redis/Mem)    │
│         │ (typed Rust RPC)   │    │  └── chartml-rs (server SVG) │
└─────────┼────────────────────┘    └──────────────────────────────┘
          └──── Network ───────────────────┘
```

**What changes:**
- REST routes replaced by server functions (type-safe, auto-serialized)
- Auth token dance eliminated (server fns run on server, auth is just an extractor)
- 5+ Context providers replaced by flat signal graph
- Two build systems become one (`cargo leptos build`)
- Two test ecosystems become one (`cargo test`)
- Server-side chart rendering becomes a Rust function call (no Node.js pod)

**What stays the same:**
- `kyomi-core` (models, DB, KV) — unchanged
- `kyomi-auth` (services, encryption, JWT) — unchanged
- `kyomi-agent` (LLM, tools, catalog) — unchanged
- `kyomi-datasource` (drivers) — unchanged
- WebSocket protocol — unchanged
- MCP server — unchanged
- Database migrations — unchanged
- Deployment (single binary) — simpler (no rust-embed dance)

---

## Migration Strategy

### Hybrid Routing (React + Leptos coexist)

```rust
// apps/server/src/lib.rs
let app = Router::new()
    // Leptos-rendered pages (new)
    .leptos_routes(&leptos_options, routes, App)
    // Existing REST API (unchanged, serves React pages)
    .nest("/api/v1", api_routes())
    // Existing WebSocket (unchanged)
    .route("/ws/{user_id}", get(ws_handler))
    // React SPA fallback (everything not yet migrated)
    .fallback(serve_react_spa);
```

Both frameworks share the same Axum server and service layer. Leptos pages use server functions (direct service calls). React pages continue using REST API. Pages migrate one at a time.

### Migration Phases

| Phase | Scope | Risk | JS Interop | Status | Detailed Plan |
|-------|-------|------|------------|--------|---------------|
| **0** | Infrastructure — `kyomi-ui` crate, Leptos+Axum integration, Tailwind, hybrid routing | Low | None | **Complete** | — |
| **1** | **Settings** (8 tabs, ~4,500 LOC) — widest component range | Low | None | **Complete** | [`LEPTOS_SETTINGS_PLAN.md`](LEPTOS_SETTINGS_PLAN.md) |
| **2** | **Auth** — Login, Signup, Verify Email, Password Recovery, Passkey flows | Medium | None | **Complete** | — (implemented directly) |
| **3** | **Watches** — watch management, AI creation, cron scheduling, Gmail-style alert inbox, execution history | Low | None | **Planned** | [`LEPTOS_WATCHES_PLAN.md`](LEPTOS_WATCHES_PLAN.md) — 23 tasks, 8 phases |
| **4** | **Knowledge** — list, tree, editor | Low | None | **Planned** | [`KNOWLEDGE_LEPTOS_MIGRATION_PLAN.md`](KNOWLEDGE_LEPTOS_MIGRATION_PLAN.md) |
| **5** | **Chat** — WebSocket streaming, chartml-rs chart rendering in responses | Medium | None | **Planned** | [`CHAT_LEPTOS_MIGRATION_PLAN.md`](CHAT_LEPTOS_MIGRATION_PLAN.md) |
| **6** | **SQL Editor** — kode-leptos SQL editor, query execution, schema browser, tabbed results | Medium | None | **Planned** | [`LEPTOS_SQL_EDITOR_PLAN.md`](LEPTOS_SQL_EDITOR_PLAN.md) — 28 tasks, 7 phases |
| **7** | **Dashboards** — kode-leptos markdown mode, chartml-rs charts, DataFusion transforms, quackstats-rs forecasting | High | None | **In Progress** | — (implemented directly on `feat/remove-react-routes`) |
| **8** | **Remaining Pages** — onboarding flows, OAuth callbacks, utility pages (Try, Connect Setup, Welcome, Unsubscribe, Accept Ownership) | Low | None | **Planned** | [`REMAINING_PAGES_LEPTOS_MIGRATION_PLAN.md`](REMAINING_PAGES_LEPTOS_MIGRATION_PLAN.md) |

### Detailed Plan Index

All detailed implementation plans live alongside this document:

| Plan | Scope | Tasks | Server Fns | Components |
|------|-------|-------|-----------|------------|
| [`LEPTOS_SETTINGS_PLAN.md`](LEPTOS_SETTINGS_PLAN.md) | Settings (8 tabs) — profile, security, workspace, datasources, analytics, billing, team, usage | 28 | ~51 | ~10 |
| [`LEPTOS_WATCHES_PLAN.md`](LEPTOS_WATCHES_PLAN.md) | Watches — CRUD, AI agent sidebar, cron scheduling, Gmail-style alerts, execution history | 23 | ~20 | ~11 |
| [`LEPTOS_SQL_EDITOR_PLAN.md`](LEPTOS_SQL_EDITOR_PLAN.md) | SQL Editor — kode-leptos editor, query execution, schema browser, tabbed results, streaming | 28 | ~12 | ~14 |
| [`LEPTOS_CHAT_PLAN.md`](LEPTOS_CHAT_PLAN.md) | Chat — session management, WebSocket streaming, markdown + chart rendering, shared ChatInterface | TBD | TBD | TBD |
| [`LEPTOS_KNOWLEDGE_PLAN.md`](LEPTOS_KNOWLEDGE_PLAN.md) | Knowledge — file list, tree navigation, markdown editor | TBD | TBD | TBD |
| [`LEPTOS_REMAINING_PAGES_PLAN.md`](LEPTOS_REMAINING_PAGES_PLAN.md) | Onboarding, OAuth callbacks, Try, Connect Setup, Welcome, Unsubscribe, Accept Ownership | TBD | TBD | TBD |

#### Why Settings First
- Exercises the **widest range of UI components** (40+ different component types)
- **No heavy dependencies** — no charts, no code editors, no real-time streaming
- **Low risk** — if settings is briefly broken, nobody is locked out (unlike Login)
- **15+ API calls** across 8 tabs — validates the server function pattern thoroughly
- Validates the full Leptos + Singlestage + Tailwind stack before committing to harder pages

#### Pages NOT Suitable for Early Migration (historical context — some now complete)
- **SQL Editor** — needed kode-leptos editor built first (now available)
- **Dashboard Editor** — needed kode-leptos + chartml-rs + DataFusion (now in progress)
- **Chat** — needed WebSocket patterns established + chartml-rs for inline charts
- **Login** — too risky as a first page (blocks all users if broken) — **now complete**

---

## Crate Structure

```
crates/
  kyomi-core/              ← unchanged
  kyomi-auth/              ← unchanged
  kyomi-agent/             ← unchanged
  kyomi-datasource/        ← unchanged
  kyomi-embed/             ← unchanged
  kyomi-knowledge/         ← unchanged
  kyomi-ui/                ← Leptos frontend
    Cargo.toml
    src/
      lib.rs               ← App root, router, server fn registration
      app.rs               ← Router with all routes
      types.rs             ← Shared types (server/client boundary)
      datasource.rs        ← KyomiDataSource (chartml-core DataSource impl)
      components/           ← Shared UI components
        mod.rs
        button.rs, input.rs, label.rs, select.rs, checkbox.rs, switch.rs
        modal.rs, card.rs, alert.rs, badge.rs, status_badge.rs
        tooltip.rs, confirm_dialog.rs, spinner.rs, skeleton.rs
        action_status.rs, theme.rs, layout.rs
        dashboard/          ← Dashboard-specific shared components
          markdown_renderer.rs, chart_builder.rs, chart_info_modal.rs
          copilot_sidebar.rs, history_panel.rs, parameters.rs
          insert_link_modal.rs, save_dashboard_modal.rs, shared.rs
      pages/
        settings/           ← COMPLETE — 8 tabs, all functional
        auth/               ← COMPLETE — login, signup, recovery, passkey, OAuth
        dashboards/         ← IN PROGRESS — list, viewer, editor
        watches/            ← PLANNED — see LEPTOS_WATCHES_PLAN.md
        knowledge/          ← PLANNED — see LEPTOS_KNOWLEDGE_PLAN.md
        chat/               ← PLANNED — see LEPTOS_CHAT_PLAN.md
        sql_editor/         ← PLANNED — see LEPTOS_SQL_EDITOR_PLAN.md
        not_implemented.rs  ← Placeholder for unmigrated routes
      server_fns/           ← Server functions (typed RPC)
        mod.rs              ← ServerContext, extract_auth, extract_context
        auth.rs, profile.rs, security.rs, dashboards.rs, datasources.rs
        collections.rs, copilot.rs, context.rs, sidebar.rs
        billing.rs, analytics.rs, team.rs, workspace.rs, usage.rs
        slack.rs            ← (feature-gated)
      utils/
        websocket.rs        ← WebSocket hook (dashboard updates, extensible)
        webauthn.rs         ← WebAuthn passkey utilities
      parser/
        chartml.rs          ← ChartML markdown parser

~/repos/kode/kode-leptos/   ← Code editor (external crate, linked via path)
```

---

## Component Library: Singlestage UI Coverage

Singlestage UI provides 40 shadcn-style components. Coverage for Settings page (Phase 1):

| Need | Singlestage Component | Available? |
|------|----------------------|-----------|
| Button | `Button` | Yes |
| Card (Header, Title, Content) | `Card` | Yes |
| Tabs | `Tabs` | Yes |
| Input | `Input` | Yes |
| Select | `Select` | Yes |
| Switch/Toggle | `Switch` | Yes |
| Dialog/Modal | `Dialog` | Yes |
| Dropdown | `Dropdown` | Yes |
| Badge | `Badge` | Yes |
| Alert | `Alert` | Yes |
| Tooltip | `Tooltip` | Yes |
| Skeleton | `Skeleton` | Yes |
| Spinner | `Spinner` | Yes |
| Toast | — | **Build custom** (Tailwind + signals) |
| Confirm Dialog | — | **Compose** from Dialog + Button |

Also available: Accordion, Avatar, Breadcrumb, Carousel, Checkbox, Context Menu, Popover, Progress, Radio, Scroll Area, Separator, Sidebar, Slider, Table, Textarea, Kbd, Pagination.

---

## Unused JS Dependencies to Remove

These are installed in `apps/frontend/package.json` but **not imported anywhere** in the frontend source:

| Package | Status |
|---------|--------|
| `ag-grid-community` | Not imported — replaced by custom `ResizableTable` |
| `ag-grid-react` | Not imported |
| `apache-arrow` | Not imported in frontend code |
| `html2canvas` | Not imported |

---

## Server-Side Chart Rendering

### Current
Requires Node.js sidecar or headless Chrome in k8s to render JS ChartML to PNG/SVG.

### With chartml-rs
```rust
// In any Rust backend route — no Node.js, no browser
let chartml = ChartML::new();
chartml.register_renderer("bar", CartesianRenderer::new());
// ... register renderers

let elements = chartml.render_from_yaml(&yaml_spec)?;
let svg_string = elements.to_svg_string(); // Pure Rust, microseconds
```

Same binary, same process, same crate. Eliminates the Node.js pod entirely.

---

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|-----------|
| Singlestage UI missing components | Low | Build with Tailwind + Basecoat CSS (framework-agnostic shadcn alternative) |
| WASM bundle size (DataFusion is large) | Medium | Code-split via lazy loading; DataFusion only loaded on pages that need it |
| Leptos ecosystem maturity vs React | Medium | Side project context — acceptable trade-off for zero-JS goal |
| kode-leptos limitations vs Monaco | Medium | Built and in use; lacks autocomplete and error squiggles, but status bar errors and catalog sidebar compensate. Can extend if needs grow. |
| DataFusion SQL dialect differences | Low | Minor syntax changes (PERCENTILE_CONT, DATE_TRUNC) |
| Migration takes too long | Low | Hybrid routing means React stays in production; no "big rewrite" risk |
| Developer pool (Leptos vs React) | Low | Currently single developer; Rust expertise already deep |

---

## Open Questions

1. **WASM bundle size budget** — DataFusion + chartml-rs + arborium + quackstats-rs all in one WASM binary. What's the total? Need to measure.
2. **Code splitting** — Can DataFusion and chartml-rs be loaded lazily (only on pages that need them)?
3. **MCP Chart App** — Currently a Vite single-file HTML app. Migrate to Leptos or keep separate?
4. **PWA / Service Worker** — Current React app has PWA support. Leptos PWA story?
5. **Shared ChatInterface** — Chat page and Watch Agent Sidebar both need a chat component. Build once in Chat migration, reuse in Watches. Watches Phase 4 depends on this.

---

## References

### Frameworks Evaluated
- [Leptos](https://github.com/leptos-rs/leptos) — 20.4K stars, v0.8.17, fine-grained signals
- [Dioxus](https://github.com/DioxusLabs/dioxus) — 35.4K stars, v0.7.3, cross-platform, YC-backed
- [Yew](https://github.com/yewstack/yew) — 32.5K stars, v0.23.0, oldest/most mature
- Sycamore (3.2K stars) — stalled, avoid
- Perseus (2.2K stars) — abandoned, avoid

### Component Libraries Evaluated
- [Singlestage UI](https://github.com/adoyle0/singlestage-ui) — 64 stars, shadcn aesthetic, Tailwind, 40 components (**chosen**)
- [Thaw UI](https://github.com/thaw-ui/thaw) — 574 stars, Fluent Design (not shadcn aesthetic)
- [Leptonic](https://github.com/lpotthast/leptonic) — 299 stars, Material-ish, own styling
- [Basecoat](https://basecoatui.com/) — framework-agnostic shadcn alternative (CSS-only fallback)

### Key Crates
- [DataFusion](https://github.com/apache/datafusion) — Pure Rust SQL engine on Arrow
- [syntect](https://github.com/trishume/syntect) — Syntax highlighting (pure Rust with `default-fancy`)
- [ropey](https://github.com/cessen/ropey) — Text rope buffer
- [comrak](https://github.com/kivikakk/comrak) — GFM markdown parser
- [augurs](https://github.com/grafana/augurs) — Time series (used by QuackStats)
- [tailwind-fuse](https://github.com/gaucho-labs/tailwind-fuse) — Rust `cn()` utility
- [arborium](https://github.com/bearcove/arborium) — Tree-sitter in WASM (alternative to syntect)

### Our Projects
- `chartml-rs` (`~/repos/chartml-rs`) — Pure Rust chart rendering, 10.6K LOC, 203 tests, full Leptos integration
- `QuackStats` (`~/repos/quackstats`) — Rust forecasting, 2.9K LOC, MIT license, 6 models
