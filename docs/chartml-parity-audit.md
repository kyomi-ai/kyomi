# ChartML Parity Audit (KYO-110)

Audit of every behavioral divergence between the two Leptos code paths that
render `chartml` fenced code blocks:

- **Markdown renderer path** — `crates/kyomi-ui/src/components/dashboard/markdown_renderer.rs`
  (`configured_chartml`, `ChartBlock`, `MarkdownRenderer`). Used by the
  dashboard viewer, the dashboard editor's **Source-mode** right-hand preview,
  the Watches viewer, and the chat message stream.
- **WYSIWYG visual-editor path** — `crates/kyomi-ui/src/components/dashboard/chartml_extension.rs`
  (`create_chartml`, `ChartMLExtension::render_code_block`, `render_one_chart`).
  Used only by the dashboard editor's **Visual-mode** Kode WYSIWYG tree
  renderer, via the `kode_leptos::extension::Extension` trait.

Chat-rendered chartml (`components/chat/copilot_chat.rs`) delegates to
`MarkdownRenderer`, so it inherits the markdown-renderer path verbatim — out
of scope for this audit beyond noting the fact.

All line references are against `main` at the worktree's base commit
(`b492139`). If Phase 2 rebases and one of these moves, the concern is still
valid — re-locate by function name.

---

## Summary table

| # | Concern | Severity | Recommended resolution |
| - | ------- | -------- | ---------------------- |
| 1 | Chart renderer registrations (types + aliases) | 🟡 inconsistent | Align on single shared list — both paths register the same 9 (type, renderer) pairs today; consolidate via a shared helper. |
| 2 | DataFusion transform registration | 🟢 cosmetic | N/A — both paths register `DataFusionTransform`. Hint was stale. |
| 3 | Palette configuration | 🔴 broken | Align to A — visual editor must read palette via `kyomi_palette(name, is_dark)`, not accept a raw `Vec<String>` that callers pre-resolve. |
| 4 | Theme configuration (`kyomi_theme`, `is_dark` source) | 🟡 inconsistent | Align to A — theme should be derived from `use_theme()` at construction time, not computed in the caller. |
| 5 | DataSource provider registration | 🟡 inconsistent | A (context-based) — already correct in both paths; document the shared reliance so refactor doesn't regress it. |
| 6 | Cache backend wiring (IndexedDB tier-2) | 🟡 inconsistent | Align to A — visual editor's `ChartMLChart`s need `.resolver().set_persistent_cache(...)` on the same `CacheBackendSignal`. |
| 7 | Resolver hooks (`tracing_hooks_ref`) | 🟡 inconsistent | Align to A — `render_one_chart` must install `tracing_hooks_ref()` on its chart's resolver. |
| 8 | YAML sequence splitting (`split_chartml_block`) | 🟢 cosmetic | N/A — both paths call `split_chartml_block`. KYO-107 already reconciled this. |
| 9 | Parameter substitution (`{{param}}`) | 🔴 broken | Make both configurable via a new shared `ParametersSignal` context; extension currently has no path to receive parameters. |
| 10 | `colSpan` layout wrapping | 🟡 inconsistent | Align to A for outer grid — viewer flattens into one 12-col grid; editor nests a separate grid per block which breaks side-by-side across blocks. |
| 11 | Chart-height reservation (no-flash loading) | 🔴 broken | Align to A — `render_one_chart` must compute `chart_height_px` and wrap `ChartMLChart` in a `min-height` div. |
| 12 | Outer CSS classes (`prose-kyomi`, `not-prose`, `dashboard-content`, `chart-card`) | 🟡 inconsistent | Keep `chart-card` (both apply it); align outer wrapper: viewer uses `prose-kyomi grid …`, editor uses `dashboard-content not-prose grid …`. Pick one pattern per host. |
| 13 | Header-bar feature set (`show_*`) | 🔴 broken | Align to A — editor should surface `show_delete`, `show_save_to_dashboard`, `show_ask_about` (conditionally), not just the 4 it shows. |
| 14 | Header-bar callback payloads | 🟡 inconsistent | Align on one payload model — either DOM events (editor pattern) everywhere, or Leptos `Callback` props everywhere. Avoid two dispatch styles. |
| 15 | `chart-edit-request` / `chart-info-request` event shape | 🟢 cosmetic | N/A — editor-only convention; viewer uses prop callbacks. Keep as-is until unified (#14). |
| 16 | Per-chart refresh signal | 🟡 inconsistent | Align to A — editor's refresh button must actually invalidate cache via `refresh_trigger`, not just bump a cosmetic timestamp. |
| 17 | Dashboard-wide refresh signal (`RefreshAllSignal`) | 🟡 inconsistent | Align to A — visual editor's `ChartMLChart`s must also fold the context `RefreshAllSignal` into `refresh_trigger`. |
| 18 | Last-refreshed timestamp source | 🔴 broken | Align to A — derive from resolver/input tick, not from first render nor from pressing the refresh button. |
| 19 | Override signals (type / orientation / mode) | 🟢 cosmetic | N/A — identical signal shapes; only store/callback plumbing differs in boilerplate. |
| 20 | `apply_spec_overrides` usage | 🟢 cosmetic | N/A — both call the same `apply_spec_overrides` helper. |
| 21 | Key generation for keyed rendering | 🟡 inconsistent | Align — viewer keys by segment enumeration index, editor has no explicit `<For>` (Kode's tree does the keying). Document; no behavior change needed. |
| 22 | Error rendering | 🟡 inconsistent | Align — both defer to `ChartMLChart`'s internal error UI, but the editor's lack of `min-height` reserve (#11) makes errors flash/reflow. |
| 23 | Loading placeholder behavior | 🔴 broken | Tied to #11 — editor has no reserved vertical space, so loading placeholder is the full height chartml picks and layout shifts on data arrival. |
| 24 | Send/Sync boundary (`SendWrapper`) | 🟢 cosmetic | A is structurally necessary (`ChartMLRef` on extension struct); B is structurally necessary (resolver in an Effect). Neither should be removed. |
| 25 | Streaming-markdown cleanup (`clean_streaming_markdown`) | 🟢 cosmetic | N/A — only `MarkdownRenderer` needs it; extension never receives incomplete blocks. |
| 26 | CodeBlock (non-chartml) rendering | 🟢 cosmetic | N/A — `MarkdownRenderer` renders `CodeBlockView` with language badge + copy; Kode's WYSIWYG renders its own non-chartml code blocks natively. |
| 27 | `workspace_id` prop auto-wiring | 🟡 inconsistent | Align — `MarkdownRenderer` installs `provide_chart_context` when the caller passes `workspace_id` and no provider is in context; `ChartMLExtension` has no such escape hatch and always relies on context. |
| 28 | Chart palette source (user preference) | 🟡 inconsistent | Align — `ChartBlock` reads `chart_palette: Option<String>` prop with `"kyomi"` fallback; extension takes a pre-resolved `Vec<String>` with no fallback if caller passes `None`. Tied to #3. |
| 29 | Per-type default chart height (`default_chart_height_for_type`) | 🔴 broken | Align to A — extension must use the same per-type default (150px metric, 400px others) to reserve loading space. Dependency of #11/#23. |
| 30 | Initial timestamp mount behavior | 🟡 inconsistent | Align — viewer seeds `last_refreshed` with `Date::now()` unconditionally at `ChartBlock` construction; extension only seeds when compiled to wasm32, leaving SSR/native at `None`. Minor SSR parity issue. |

Totals: **7 🔴 broken · 15 🟡 inconsistent · 8 🟢 cosmetic** = **30 concerns**.
Seed list had 24 items; this audit adds 6 explicit additions (#25–30), and
reclassifies #2 and #8 from stale hints to confirmed parity (both fixed in
earlier KYO-107 / transform-wiring work).

---

## Detailed entries

Every entry uses the five-field format requested by KYO-110:

1. **Markdown renderer**
2. **Visual editor**
3. **User-visible consequence**
4. **Severity**
5. **Recommended resolution**

### 1. Chart renderer registrations (types + aliases)

1. **Markdown renderer**: `markdown_renderer.rs:466-483` (`configured_chartml`) —
   registers `bar/line/area` (Cartesian), `pie/donut/doughnut` (Pie),
   `scatter`, `metric`, `table`. **All three pie aliases** (`pie`, `donut`,
   `doughnut`) are wired.
2. **Visual editor**: `chartml_extension.rs:31-50` (`create_chartml`) —
   registers the **same** 9 `(type, renderer)` pairs in the same order,
   including both `donut` and `doughnut`.
3. **Consequence**: None functional today. The seed-list hint "doughnut only"
   vs "donut only" is **stale** — both paths register both aliases after
   recent renderer work.
4. **Severity**: 🟡 inconsistent (duplication across two call sites is a
   maintenance hazard even though behavior matches right now).
5. **Resolution**: Extract `register_kyomi_renderers(&mut ChartML)` into
   `kyomi-chart-theme` (or a new `kyomi-chart-factory` helper) and call from
   both paths. Single source of truth for "which aliases does Kyomi support".

### 2. DataFusion transform registration

1. **Markdown renderer**: `markdown_renderer.rs:479` — `c.register_transform(DataFusionTransform);`.
2. **Visual editor**: `chartml_extension.rs:42` — `chartml.register_transform(DataFusionTransform);`.
3. **Consequence**: None — both paths register it. The seed-list hint
   "not registered in extension" is **stale**.
4. **Severity**: 🟢 cosmetic.
5. **Resolution**: N/A. Picks up the shared-factory consolidation from #1
   automatically.

### 3. Palette configuration

1. **Markdown renderer**: `markdown_renderer.rs:790-794` — reads
   `chart_palette: Option<String>` prop (default `"kyomi"`), derives
   `is_dark` from `use_theme()`, then calls `kyomi_palette(name, is_dark)`
   inside `configured_chartml`.
2. **Visual editor**: `chartml_extension.rs:84-88` — caller
   (`dashboard_editor.rs:1268-1272`) passes a pre-resolved
   `Vec<String>` via `ChartMLExtension::with_colors_and_theme`. If the caller
   passes `None`, `ChartMLExtension::new()` is used which registers NO
   palette at all (`chartml_extension.rs:71`).
3. **Consequence**: When the editor has no user palette preference, the
   visual-mode charts render with the chartml default palette instead of
   Kyomi's default Kyomi palette. Charts look visibly different between
   source-preview and WYSIWYG mode on a fresh account.
4. **Severity**: 🔴 broken — visual regression between modes on the same page.
5. **Resolution**: Align to A. `create_chartml` should accept
   `palette_name: &str` (default `"kyomi"`) and call `kyomi_palette` itself.
   Drop the pre-resolved `Vec<String>` from the extension's public API.

### 4. Theme configuration (`kyomi_theme`, `is_dark` source)

1. **Markdown renderer**: `markdown_renderer.rs:791-794` — reads `is_dark`
   from `use_theme()` inside `ChartBlock`, passes into
   `configured_chartml(&palette, is_dark)` which calls
   `kyomi_theme(is_dark)`. `is_dark` is read `get_untracked()`, **not**
   reactive — a live theme toggle won't recolor existing charts until they
   re-mount.
2. **Visual editor**: `dashboard_editor.rs:1263-1267` — reads `is_dark` via
   `use_theme()` at the **page-owner scope** (outside the extension),
   computes `kyomi_theme(is_dark)`, and passes the pre-computed `Theme` into
   `ChartMLExtension::with_colors_and_theme`. Because the extension instance
   is cloned and held by Kode for its lifetime, a theme toggle won't recolor
   any existing chart, and new charts created after the toggle ALSO don't
   pick it up until the editor remounts.
3. **Consequence**: Both paths fail to hot-swap theme on toggle, but the
   editor's failure is worse — theme is baked at extension construction, not
   at chart construction, so even re-parsing the chart YAML doesn't help.
4. **Severity**: 🟡 inconsistent (both buggy; editor is the more permanent
   failure).
5. **Resolution**: Align to A, then go one step further — make theme
   reactive by reading `use_theme()` inside a `Memo` that drives the
   `ChartMLRef`'s `set_theme` via `Effect`. Fix both paths in the same
   refactor.

### 5. DataSource provider registration

1. **Markdown renderer**: `markdown_renderer.rs:784-789` comment documents
   that provider comes from context (`DashboardChartProviders` wraps the
   viewer; `MarkdownRenderer` also auto-installs via `provide_chart_context`
   when the caller passes `workspace_id` and no provider exists — see
   `markdown_renderer.rs:1086-1090`). Never registers a per-chart provider.
2. **Visual editor**: Same — `chartml_extension.rs` never calls
   `register_data_source` on its `ChartML` instance. The `ChartMLChart` in
   `render_one_chart` reads `ProviderRef` from context. Editor's
   `dashboard_editor.rs:704` calls `provide_chart_context(&workspace_id.get())`
   at page-owner scope, so context is visible to both Source-mode
   `MarkdownRenderer` and Visual-mode Kode-extension renders.
3. **Consequence**: None — already aligned.
4. **Severity**: 🟡 inconsistent (implicit dependency on context is a
   refactor hazard — drop `provide_chart_context` and BOTH paths silently
   break for remote data).
5. **Resolution**: Document the context dependency in a doc-comment on
   `create_chartml` / `configured_chartml`. Add a debug-only
   `debug_assert!(use_context::<ProviderRef>().is_some())` inside `ChartBlock`
   / `render_one_chart` so a missing provider surfaces immediately rather
   than deferring to an opaque "no datasource" resolver error.

### 6. Cache backend wiring (IndexedDB tier-2)

1. **Markdown renderer**: `markdown_renderer.rs:815-825` reads
   `CacheBackendSignal` from context and installs it on the chart's resolver
   via `resolver.set_persistent_cache(backend)` inside an `Effect`. The
   effect re-runs when the signal flips `None → Some(backend)`, so charts
   that mounted before IDB finished opening pick up the tier-2 cache as soon
   as it's ready.
2. **Visual editor**: `chartml_extension.rs:176-333` (`render_one_chart`) —
   **no cache backend wiring**. The resolver gets the default in-memory
   tier-1 cache only. No `use_context::<CacheBackendSignal>()`, no
   `set_persistent_cache`.
3. **Consequence**: Charts in visual-edit mode re-fetch on every render even
   for cross-session cached specs. Noticeable on slow datasources; invisible
   for inline-row specs.
4. **Severity**: 🟡 inconsistent — correctness is fine, performance and
   network cost degrade.
5. **Resolution**: Align to A. Copy the `Effect::new` cache-hydrate block
   from `ChartBlock` into `render_one_chart`. Trivial — just needs a
   `chartml` local and a `use_context::<CacheBackendSignal>()` read.

### 7. Resolver hooks (progress, error, loading callbacks)

1. **Markdown renderer**: `markdown_renderer.rs:802-804` —
   `chartml.resolver().set_hooks(tracing_hooks_ref())` installs the shared
   `TracingHooks` impl that logs every fetch/transform phase through
   `tracing::`.
2. **Visual editor**: `render_one_chart` never calls `set_hooks`. Fetches
   inside the editor are silent in the tracing log.
3. **Consequence**: Debugging a failed chart in visual-edit mode is
   materially harder than in the viewer — the same spec that logs full
   phases in the viewer produces no telemetry in the editor.
4. **Severity**: 🟡 inconsistent.
5. **Resolution**: Align to A — add `chartml.resolver().set_hooks(tracing_hooks_ref());`
   to `render_one_chart` alongside the cache hydrate fix (#6).

### 8. YAML sequence splitting (`split_chartml_block`)

1. **Markdown renderer**: `markdown_renderer.rs:230-245` defines
   `split_chartml_block`, called at `markdown_renderer.rs:121,150`.
2. **Visual editor**: `chartml_extension.rs:116` calls
   `split_chartml_block(content.trim())` — imported at
   `chartml_extension.rs:25`.
3. **Consequence**: None — both paths split identically. The seed-list
   hint "not split in extension" is **stale** (KYO-107 fixed this).
4. **Severity**: 🟢 cosmetic.
5. **Resolution**: N/A.

### 9. Parameter substitution (`{{param}}`)

1. **Markdown renderer**: `markdown_renderer.rs:775-780` folds a
   `parameters: Signal<HashMap<String, String>>` prop through
   `substitute_params` (defined at `markdown_renderer.rs:324-330`) and
   feeds the substituted YAML to `ChartMLChart`. The dashboard viewer
   supplies the signal at `dashboard_viewer.rs:976`.
2. **Visual editor**: Parameter substitution is not performed at all.
   `chartml_extension.rs:214-230` builds `effective_spec` from overrides
   only; `substitute_params` is not imported, and the extension has no
   Leptos prop or context channel through which dashboard parameters
   could flow. `dashboard_editor.rs` also never tracks `param_values` —
   the source-mode `MarkdownRenderer` at `dashboard_editor.rs:896`
   likewise omits `parameters=…`.
3. **Consequence**: A chart spec using `{{region}}` in its SQL renders
   fine in the viewer but fails in the editor preview (both modes) because
   the placeholder reaches the datasource verbatim. Authors see a broken
   chart during editing; saving and loading the viewer "fixes" it.
4. **Severity**: 🔴 broken — core authoring loop is broken for any
   template-based dashboard.
5. **Resolution**: Make both configurable via a shared
   `ParametersSignal = Signal<HashMap<String, String>>` in Leptos context.
   Dashboard editor provides it (with preview values — either empty, or
   surfaced in a new editor-side parameters panel) so BOTH the source-mode
   `MarkdownRenderer` AND the visual-mode `ChartMLExtension` pick it up.
   Extension would read it in `render_one_chart` before feeding the spec
   into `ChartMLChart`. **Judgment call**: editor UX for parameter
   preview-values is out of scope of this audit; flag for KYO-109 /
   ticket author decision.

### 10. `colSpan` layout wrapping

1. **Markdown renderer**: `markdown_renderer.rs:1120` wraps the entire
   content in **one** `prose-kyomi grid grid-cols-12 gap-4` div and emits
   every chart as a direct `col-span-*` child. Two `colSpan: 6` charts in
   separate ```` ```chartml ```` fences share one row (test:
   `test_adjacent_chartml_blocks_with_col_span`).
2. **Visual editor**: `chartml_extension.rs:155` wraps **each block** in
   its own `dashboard-content not-prose grid grid-cols-12 gap-4` div.
   Multiple blocks in the editor are sibling grids, so two `colSpan: 6`
   charts in separate fences always stack full-width — they cannot share
   a row.
3. **Consequence**: A dashboard with two side-by-side charts in separate
   fences looks correct in the viewer and in source-mode preview, but
   stacks vertically in visual mode. Changes the visual layout
   perceptibly between edit modes.
4. **Severity**: 🟡 inconsistent — both still render every chart with its
   own col-span class; only the outer grid differs.
5. **Resolution**: Align to A. However, the Kode extension interface
   (`render_code_block` returns one view per block) makes a
   cross-block parent grid hard — Kode inserts its own DOM between each
   block's output. This is a **judgment call**: either (a) accept the
   divergence as a Kode limitation, or (b) push a parent-grid wrapper
   into Kode itself. Recommend tracking separately from the main refactor.

### 11. Chart-height reservation (`extract_chart_height`, wrapper min-height)

1. **Markdown renderer**: `markdown_renderer.rs:692-695` calls
   `extract_chart_height` (YAML `visualize.style.height`) with a per-type
   fallback via `default_chart_height_for_type` (150px for metric, 400px
   for others). Wraps `ChartMLChart` in
   `<div class="w-full" style="min-height: Npx">` at
   `markdown_renderer.rs:1001`.
2. **Visual editor**: `chartml_extension.rs:291-330` (`render_one_chart`)
   wraps the chart in a `chart-card` div with no computed `min-height`.
3. **Consequence**: Before chart data arrives, the visual-edit
   `ChartMLChart` placeholder collapses to its intrinsic size; once data
   resolves and the SVG paints, the surrounding content reflows
   noticeably. Layout shift is especially visible when typing above a
   chart in visual mode.
4. **Severity**: 🔴 broken — regression of KYO-118 (metric layout-shift
   fix) in a different code path.
5. **Resolution**: Align to A. Port the `chart_height_px` derivation into
   `render_one_chart` and wrap the chart with the same `min-height` div.
   Depends on extracting `default_chart_height_for_type` to be callable
   from both modules (it's `pub(crate)` in `markdown_renderer.rs` today).

### 12. Outer CSS classes

1. **Markdown renderer**: Outermost wrapper is
   `prose-kyomi grid grid-cols-12 gap-4 [extra_class]`
   (`markdown_renderer.rs:1120`). Each chart is wrapped in
   `class="chart-card"` (`markdown_renderer.rs:943`). Non-chart code
   blocks use bespoke `<pre><code>`.
2. **Visual editor**: Outermost wrapper is
   `dashboard-content not-prose grid grid-cols-12 gap-4`
   (`chartml_extension.rs:155`). Each chart is wrapped in
   `class="chart-card"` (`chartml_extension.rs:292`).
3. **Consequence**:
   - `prose-kyomi` (viewer) vs `not-prose` (editor) — prose typography
     inside markdown paragraphs renders differently. Seed-list hint is
     partly stale: viewer uses `prose-kyomi` (no `not-prose`); editor uses
     `dashboard-content not-prose`. Both diverge.
   - `dashboard-content` class (editor only) is a selector that cascades
     `chart-card` border / radius / bg — but the viewer also applies it
     higher up (`dashboard_viewer.rs:~973` inside `DashboardChartProviders`
     subtree — note: verify during refactor). If the viewer relies on an
     ancestor for `dashboard-content` and the editor's extension bakes it
     in, a viewer refactor that drops the ancestor silently breaks
     viewer chart-card styling.
4. **Severity**: 🟡 inconsistent.
5. **Resolution**: Pick one pattern per *host* (page). The extension
   shouldn't own `dashboard-content` — it's a page-level concern. Move
   the class to the dashboard editor's page wrapper (same as the viewer)
   and have the extension emit only `grid grid-cols-12 gap-4`. Keep
   `chart-card` where it is.

### 13. Header-bar feature set (`show_*`)

1. **Markdown renderer**: `markdown_renderer.rs:971-977` sets
   `show_type_selector=true`, `show_refresh=true`, `show_edit=has_edit`,
   `show_delete=has_delete`, `show_save_to_dashboard=has_save`,
   `show_info=has_info`, `show_ask_about=has_ask`. `has_*` are derived
   from whether the respective `on_*` callback was passed by
   `MarkdownRenderer`'s caller.
2. **Visual editor**: `chartml_extension.rs:311-314` sets
   `show_type_selector=true`, `show_refresh=true`, `show_info=true`,
   `show_edit=true`. **`show_delete`, `show_save_to_dashboard`, and
   `show_ask_about` are not set** (defaults to `false` per their
   `#[prop(optional)]`).
3. **Consequence**: In visual-edit mode, charts have no "Delete",
   "Save to Dashboard", or "Ask about" buttons. Users editing a
   dashboard in Visual mode cannot delete a chart via its header —
   they have to switch to Source mode, find the fence, delete the
   text manually. Seed-list hint was accurate.
4. **Severity**: 🔴 broken — core authoring action is unavailable in
   Visual mode.
5. **Resolution**: Align to A. Add Delete (primary) and Save/Ask
   (if the editor context makes them meaningful — they might not,
   since you're already *in* the dashboard) as `show_*` + `on_*`
   callbacks wired through DOM events similar to the existing
   `chart-edit-request`. Specifically: dispatch a new
   `chart-delete-request` event carrying `{block_content, array_index}`;
   editor listener uses `splice_chartml_item` logic to remove the item.

### 14. Header-bar callback wiring — payloads

1. **Markdown renderer**: `markdown_renderer.rs:870-929` — every action
   is a typed Leptos `Callback<T>`: edit/delete pass `(block_index,
   array_index)`, save/ask pass YAML wrapped in a ```` ```chartml ````
   fence, info passes raw YAML, refresh bumps a local `RwSignal<u32>`.
   Callbacks flow as `MarkdownRenderer` props down to `ChartBlock`.
2. **Visual editor**: `chartml_extension.rs:244-281` — edit/info
   dispatched as browser `CustomEvent` with string `detail`
   (`dispatch_chart_info_event`, `dispatch_chart_edit_event`).
   Edit payload is a JSON-stringified `{yaml, block_content, array_index}`.
   Info payload is a raw YAML string. Refresh is cosmetic-only (#16).
3. **Consequence**: Two dispatch mechanisms in one codebase. Adding a
   new action (e.g. Delete, see #13) requires picking a side. Kode can't
   trivially pass Leptos callbacks down into a cloned extension, so the
   event-dispatch side is forced by the Kode API. But it means
   `dashboard_editor.rs` has hand-rolled event listeners
   (`dashboard_editor.rs:535-548, 605-618`) parallel to
   `dashboard_viewer.rs`'s callback prop wiring.
4. **Severity**: 🟡 inconsistent.
5. **Resolution**: Accept the difference as a Kode API constraint;
   factor the event-listener setup into a reusable helper
   (`install_chart_event_listeners(on_edit, on_info, on_delete, …)`)
   so adding a new action only needs a single site change in Kode-host
   code.

### 15. `chart-edit-request` / `chart-info-request` event payload shape

1. **Markdown renderer**: N/A — uses typed callbacks, no DOM events.
2. **Visual editor**: `chart_edit_request` detail is JSON-stringified
   `{yaml: string, block_content: string, array_index: usize}`.
   `chart-info-request` detail is a raw YAML string.
3. **Consequence**: Two different event serializations on the same
   event pattern. Listener has to `ev.detail()` as a string and then
   conditionally `JSON.parse` only for `chart-edit-request`.
4. **Severity**: 🟢 cosmetic.
5. **Resolution**: If #14's consolidation lands, both events become
   consistent JSON payloads (`{yaml, block_content, array_index}`).

### 16. Per-chart refresh signal

1. **Markdown renderer**: `markdown_renderer.rs:708, 886-888, 1005` —
   `local_refresh: RwSignal<u32>` is bumped by the Refresh button and
   folded into a `combined_refresh` Signal that becomes
   `ChartMLChart`'s `refresh_trigger` prop. The chartml component
   invalidates resolver cache keys and re-fetches.
2. **Visual editor**: `chartml_extension.rs:269-281` — Refresh button
   only updates `last_refreshed` (via `js_sys::Date::now()`). **No
   `refresh_trigger` is passed to `ChartMLChart`.** Cache is not
   invalidated; no re-fetch happens. The only visible effect is the
   header timestamp changing.
3. **Consequence**: Users clicking Refresh in visual-edit mode see the
   timestamp update but no new data is fetched. Stale data appears
   fresh. Cosmetic lie on top of a real failure.
4. **Severity**: 🟡 inconsistent (on its own; tied together with #17
   and #18 the combined effect is more severe).
5. **Resolution**: Align to A. Add `refresh_trigger: Signal<u32>` prop
   wiring inside `render_one_chart`, bumped by the Refresh callback.

### 17. Dashboard-wide refresh signal (`RefreshAllSignal`)

1. **Markdown renderer**: `markdown_renderer.rs:717-729` reads
   `use_context::<RefreshAllSignal>()` (optional) and folds its value
   into `combined_refresh`. The dashboard viewer provides it at
   `dashboard_viewer.rs:235-236`.
2. **Visual editor**: Not read. `render_one_chart` never calls
   `use_context::<RefreshAllSignal>()`. Even if the dashboard editor
   provided a `RefreshAllSignal`, it wouldn't propagate into
   visual-edit charts.
3. **Consequence**: No "Refresh All" UI exists in the dashboard editor
   today, but once one does (inevitable for feature parity with the
   viewer), the visual-edit charts silently ignore it.
4. **Severity**: 🟡 inconsistent — would fail the moment the editor gets
   a refresh-all button.
5. **Resolution**: Align to A. Reading `use_context` is free and safe
   (`None` in contexts that don't provide it). Fold into the
   `refresh_trigger` signal alongside the per-chart one, same pattern
   as `ChartBlock`.

### 18. Last-refreshed timestamp source

1. **Markdown renderer**: `markdown_renderer.rs:840-850` — seeded with
   `Some(js_sys::Date::now())` at mount, then updated inside an Effect
   that subscribes to `chartml_spec_signal`, `combined_refresh`, and
   `parameters`. Timestamp advances when the fetch pipeline actually
   re-runs.
2. **Visual editor**: `chartml_extension.rs:261-281` — initialized to
   `None`, then set to `js_sys::Date::now()` once at mount (inside
   `#[cfg(target_arch = "wasm32")]`) and again on every Refresh-button
   click. **Not** driven by spec changes, not by cache invalidation,
   not by parameter changes (which don't exist here anyway — see #9).
3. **Consequence**: The header's "Last refreshed X ago" label in
   visual-edit mode is decorative, not informative. Combined with #16
   (refresh is cosmetic), the timestamp is wrong in both
   "I'm showing you when data came in" and "I updated the timestamp
   for you because you clicked Refresh" senses.
4. **Severity**: 🔴 broken — user-visible misinformation.
5. **Resolution**: Align to A. After #16 is fixed, reuse the same
   `Effect` that subscribes to spec/refresh/params — the timestamp
   source becomes "whenever anything would trigger a fetch."

### 19. Override signals (type / orientation / mode)

1. **Markdown renderer**: `markdown_renderer.rs:697-700` — three
   `signal()` pairs for `Option<String>` / `Option<Option<String>>`
   override state, derived `Memo`s for current values (L732-750).
2. **Visual editor**: `chartml_extension.rs:190-211` — identical
   pattern: three `signal()` pairs with the same shape, identical
   `Memo` structure.
3. **Consequence**: None — logically equivalent.
4. **Severity**: 🟢 cosmetic.
5. **Resolution**: Lift into a `use_chart_overrides(initial_spec) ->
   ChartOverrideState` helper shared by both paths during the
   refactor. Deduplicates ~30 lines per path.

### 20. `apply_spec_overrides` usage

1. **Markdown renderer**: `markdown_renderer.rs:767-773` inside the
   `chartml_spec_signal` Memo.
2. **Visual editor**: `chartml_extension.rs:224-229` inside the
   `effective_spec` Memo. Both import the same `pub(crate)` function
   from `markdown_renderer`.
3. **Consequence**: None — identical. Single function, two call sites.
4. **Severity**: 🟢 cosmetic.
5. **Resolution**: N/A. Will move to the shared helper recommended
   by #19.

### 21. Key generation for keyed rendering

1. **Markdown renderer**: `markdown_renderer.rs:1121-1125` — outer
   `<For>` keys by segment-enumeration index (`(i, _) | *i`).
   Per-chart children iterate with `.enumerate()` but emit directly
   into the outer grid without a keyed `<For>` around the yaml list.
2. **Visual editor**: No explicit `<For>` inside `render_code_block`.
   Kode's own tree renderer keys each code block by its position in
   the document (see `kode-leptos/.../doc_renderer.rs`), so per-chart
   identity is effectively "whichever block was at this byte-range
   last render".
3. **Consequence**: In the viewer, a markdown segment inserted before
   a chart block causes `block_index` to shift and every subsequent
   chart re-mounts (losing override / refresh state). The editor's
   Kode path has the same behaviour via Kode's own keying. Both paths
   share the brittleness, just for different reasons.
4. **Severity**: 🟡 inconsistent — behaviorally equivalent, but the
   two mechanisms mean a refactor that changes the viewer's keying
   won't automatically improve the editor, and vice versa.
5. **Resolution**: Document the shared brittleness. Out of scope for
   KYO-109 unless it chooses to stabilize chart identity by a
   content-hash or `id:` YAML field.

### 22. Error rendering

1. **Markdown renderer**: All error surfaces delegated to
   `ChartMLChart`'s internal error UI (the Spec/Fetch/Transform phases
   surface errors in-component). `ChartBlock` wraps it in a
   `chart-card` with `min-height` (#11), so the error banner sits
   inside a reserved box.
2. **Visual editor**: Same delegation — `render_one_chart` wraps
   `ChartMLChart` in `chart-card` but without `min-height`. An
   error banner shows up at its intrinsic size and the editor content
   reflows around it.
3. **Consequence**: Errors in visual-edit mode look jumpier — the
   chart card expands/contracts as the state transitions
   `Loading → Error → Loaded`. Same text, same banner, worse UX.
4. **Severity**: 🟡 inconsistent — really a symptom of #11.
5. **Resolution**: Fix #11 (reserve height); error rendering follows
   automatically.

### 23. Loading placeholder behavior

1. **Markdown renderer**: `ChartMLChart`'s internal loading state renders
   inside a `min-height`-reserved div (`markdown_renderer.rs:1001`), so
   layout is stable while data resolves.
2. **Visual editor**: Same internal loader, but the wrapper has no
   reserved height (#11). Layout is unstable.
3. **Consequence**: Visual-mode dashboards look janky on first paint
   and on any cache miss.
4. **Severity**: 🔴 broken — dependency of #11.
5. **Resolution**: See #11 / #29.

### 24. Send/Sync boundary (`SendWrapper` usage)

1. **Markdown renderer**: `markdown_renderer.rs:816` wraps the resolver
   in `SendWrapper` specifically to satisfy the `Send + Sync` constraint
   on `Effect::new` closures in Leptos. Comment at L812-814 spells it
   out.
2. **Visual editor**: `chartml_extension.rs:59` wraps `ChartMLRef`
   (on the `ChartMLExtension` struct itself) in `SendWrapper` because
   `Extension: Send + Sync` and `ChartMLRef` is `Rc<ChartML>` on
   wasm32. Inside `render_one_chart`, `SendWrapper` is NOT used — the
   `ChartMLRef` is dereferenced and cloned immediately, keeping the
   reactive closures' captures `!Send`, which Leptos tolerates at
   component-body scope (no Effect around them).
3. **Consequence**: None — both uses are structurally necessary at the
   boundary they exist at.
4. **Severity**: 🟢 cosmetic.
5. **Resolution**: N/A — do not remove in refactor. The extension
   needs `SendWrapper` on the struct because of the Extension trait
   bound; the renderer needs it on the resolver because of the Effect
   bound. Different reasons, both load-bearing.

### 25. Streaming-markdown cleanup (`clean_streaming_markdown`)

1. **Markdown renderer**: `markdown_renderer.rs:67-84` strips
   unterminated ```` ```chartml ```` fences from streaming content so
   partial YAML doesn't reach the parser. `MarkdownRenderer` toggles
   it via `is_streaming: Option<Signal<bool>>`. Used by the chat
   message stream (`copilot_chat.rs:197`).
2. **Visual editor**: No equivalent. The extension never receives
   streaming content — Kode hands it complete, committed fence
   content.
3. **Consequence**: None — streaming is a concern of the host,
   not the renderer.
4. **Severity**: 🟢 cosmetic.
5. **Resolution**: N/A. The streaming code stays in
   `markdown_renderer.rs`; extension has no parallel need.

### 26. CodeBlock (non-chartml) rendering

1. **Markdown renderer**: `markdown_renderer.rs:491-552` renders
   non-chartml fences with a `CodeBlockView` component — language
   badge top-left, copy button top-right, `<pre><code>`.
2. **Visual editor**: Out of scope — `ChartMLExtension::code_block_languages()`
   returns `&["chartml"]`, so non-chartml blocks fall through to Kode's
   built-in code-block rendering (syntax highlighting via
   `kode-markdown`, no copy button).
3. **Consequence**: None — different rendering, but different
   renderers have different responsibilities.
4. **Severity**: 🟢 cosmetic.
5. **Resolution**: N/A. If visual consistency between source-preview
   and WYSIWYG ever becomes a goal for code fences, Kode would need
   a copy-button hook, not the extension.

### 27. `workspace_id` prop auto-wiring

1. **Markdown renderer**: `markdown_renderer.rs:1073-1090` — accepts
   `workspace_id: String` prop. When non-empty AND no `ProviderRef`
   exists in context, calls `provide_chart_context` locally. Lets hosts
   like Watches (`KYO-119`) embed `MarkdownRenderer` outside a
   `DashboardChartProviders` wrapper without a per-call-site patch.
2. **Visual editor**: No equivalent. `ChartMLExtension` assumes its
   ancestor has already installed a provider. If used outside a
   page that calls `provide_chart_context`, remote-data charts will
   silently fail.
3. **Consequence**: Not user-visible today — the extension is only
   used inside `dashboard_editor.rs` which always installs the
   provider. But any future host that wants to mount a Kode WYSIWYG
   with chartml blocks (e.g. Knowledge editor, chat composer, etc.)
   has to remember to wire `provide_chart_context` itself.
4. **Severity**: 🟡 inconsistent — future hazard, not present bug.
5. **Resolution**: Align. Add an `ChartMLExtension::with_workspace_id`
   constructor that stashes the id, and call `provide_chart_context`
   inside `render_one_chart` the first time it runs without a
   provider in context. Same escape hatch, same semantics.

### 28. Chart palette source (user preference)

1. **Markdown renderer**: `markdown_renderer.rs:790` reads a
   `chart_palette: Option<String>` prop, falls back to `"kyomi"` when
   missing. Viewer/editor source-mode pass the user's preference
   through from `UserContext`.
2. **Visual editor**: `chartml_extension.rs:75-88` accepts a
   pre-resolved `Vec<String>`. No string-name API. No fallback to
   `kyomi_palette("kyomi", …)` when `chart_colors` is None at the
   editor call site (`dashboard_editor.rs:1268-1272`) — falls through
   to `ChartMLExtension::new()` which registers no palette at all.
3. **Consequence**: When the user has no palette preference set, the
   editor's visual-mode charts render with chartml's built-in default
   palette (not Kyomi's), while the viewer and source-mode preview
   render with Kyomi's default. Tied to #3.
4. **Severity**: 🟡 inconsistent.
5. **Resolution**: Tied to #3 — accept `palette_name: &str` directly.

### 29. Per-type default chart height (`default_chart_height_for_type`)

1. **Markdown renderer**: `markdown_renderer.rs:392-416` —
   `default_chart_height_for_type(Some("metric"))` returns 150px;
   everything else 400px. Tests pin this at
   `markdown_renderer.rs:1507-1525`. Used by `ChartBlock` as the
   fallback when the spec omits an explicit `visualize.style.height`.
2. **Visual editor**: Not called. No per-type reservation; layout
   relies on the intrinsic size of the rendered chart.
3. **Consequence**: Metric cards (150px true height) reserve 0px in
   visual mode and reflow aggressively. Cartesian / pie / etc. (400px
   true height) similarly cause layout shift. This is the specific
   KYO-118 bug, but in the visual-edit path.
4. **Severity**: 🔴 broken — regression of KYO-118 in a different
   file.
5. **Resolution**: Reuse `default_chart_height_for_type` in
   `render_one_chart`. It's already `pub(crate)` in
   `markdown_renderer.rs`. Consider hoisting to a shared module during
   refactor so neither path owns it. Dependency of #11.

### 30. Initial timestamp mount behavior

1. **Markdown renderer**: `markdown_renderer.rs:840` — seeded with
   `Some(js_sys::Date::now())` unconditionally at component construction
   (no `cfg(target_arch = "wasm32")` gate at the init site; the call
   to `js_sys::Date::now()` will fail to compile on non-wasm targets,
   but `ChartBlock` is wasm-only in practice).
2. **Visual editor**: `chartml_extension.rs:261, 265-268` — seeded to
   `None`, then set to `Date::now()` inside a
   `#[cfg(target_arch = "wasm32")]` block. Non-wasm targets leave it
   at `None`.
3. **Consequence**: On SSR / native builds, the editor's header bar
   shows no "Last refreshed" text at all until a refresh click. The
   viewer's path will crash-compile on non-wasm (line 840), but
   cross-target compilation is not exercised in practice for either
   path. Minor SSR parity issue.
4. **Severity**: 🟡 inconsistent.
5. **Resolution**: Align on "None initially; set on first successful
   data resolution" — neither path should fake a timestamp at mount
   before data has arrived. Requires a hook into `ChartMLChart`'s
   resolved signal (tied to #18).

---

## Judgment calls flagged for the ticket author

These divergences have a non-obvious "correct" answer. I've recorded the
facts; Phase 2 should decide.

1. **#9 (parameter substitution)** — the extension has no current path
   to receive dashboard parameters, and the editor doesn't track any.
   Fixing it fully requires a new `ParametersSignal` context AND an
   editor-side UI for preview-values. Scope of UI is the judgment call.
2. **#10 (cross-block colSpan grid)** — Kode's `render_code_block`
   emits per-block DOM that Kode itself wraps. A parent grid that
   spans multiple blocks needs either a Kode change or accepting the
   divergence. Recommend: accept as Kode limitation, document.
3. **#13 (header-bar feature set)** — specifically "Save to Dashboard"
   and "Ask about this chart" in visual-edit mode. In the viewer
   these are meaningful because the user is a consumer; in the editor
   they might be redundant (you're already editing the dashboard).
   Recommend: add Delete; leave Save / Ask for the ticket author's
   call.
4. **#14 (dispatch mechanism)** — keeping two dispatch styles is
   sustainable if the event-side is factored into one helper. Fully
   unifying on one style is a larger change that touches Kode's API.

---

## Out of scope (confirmed, not audited)

- Dashboard viewer-specific page features — refresh-all button wiring
  at the page level, export pipelines, print view CSS.
- Chat-rendered chartml (`components/chat/*.rs`) — delegates verbatim
  to `MarkdownRenderer`, inherits path A in full.
- Chart-builder standalone preview (`components/dashboard/chart_builder.rs`) —
  uses `ChartMLChart` directly without either wrapper.
- Any proposed Phase 2 implementation strategy — that's KYO-109.

---

## Acceptance-criteria crosswalk

- [x] Audit doc at `docs/chartml-parity-audit.md`
- [x] Every seed-list concern addressed (see #1–#24 for the 24 seeds;
      several reclassified from stale hints)
- [x] At least 3 additional divergences beyond the seed list — 6
      explicit additions (#25 streaming-cleanup, #26 non-chartml
      code-block rendering, #27 workspace_id auto-wiring, #28
      palette-source API, #29 per-type default height reuse, #30
      initial-timestamp cfg gating)
- [x] Severity rating present on every entry
- [x] Recommended resolution present on every entry
- [x] `cargo check --workspace` clean (trivially — no code changes in
      this branch)
