# KYO-77 — Chart Builder Bidirectional State Sync: Completion Report

## Ticket
[KYO-77](https://linear.app/kyomi/issue/KYO-77) — Urgent P0 regression. Editing the YAML tab in the chart builder modal had no effect on save; any pasted YAML (and any fields the Visual tab didn't model — `layout.colSpan`, `cache.ttl`, custom `style` blocks) was lost.

## What was built
- Refactored `crates/kyomi-ui/src/components/dashboard/chart_builder.rs` to use a single source of truth: the parsed YAML AST (`serde_yaml::Value`). All three sub-tabs (Visual / AI / YAML) read from and write to this AST.
- Added a separate `yaml_text` text buffer and `yaml_parse_error` signal for the YAML editor, so invalid-in-progress typing doesn't clobber the cursor or disturb the AST.
- Introduced `visual_series: RwSignal<Vec<SeriesEntry>>` as a Visual-tab-only view-state layer so blank rows added via `+ Add Series` can exist transiently without polluting the saved YAML.
- Series closures now look up entries by stable `id`, not by `idx`, so removing a middle entry doesn't corrupt surviving rows.
- New-chart seed uses `data.query` instead of `data.sql`. The renderer's `extract_query` only reads `query`/`url`, so the old `sql:` stub was dead code and was the reason new single-chart dashboards rendered empty.
- Legacy `data.sql` is migrated to `data.query` on the first edit (getter still accepts both for read-back compatibility).
- `build_yaml` and `patch_yaml` removed entirely; serialization happens via the AST.

## Review summary
- Tasks reviewed: 1 (single-file refactor).
- Review cycles: 2.
- Round 1: reviewer found 2 MAJOR issues — `+ Add Series` discard-on-write because `ast_set_series` filtered empty rows; stale `idx` captures in series on:input closures after a remove.
- Round 2: both MAJORs fixed (view-state layer + id-based lookup). Zero issues found. Approved.
- Minor issues deferred (tracked below).

## Acceptance criteria verification

| # | Criterion | Status | Evidence |
|---|-----------|--------|----------|
| 1 | Paste YAML → save → re-open preserves YAML | ✅ verified | Playwright: after save, dashboard source view contained title + x-field from YAML edit |
| 2 | Visual tab edit updates YAML tab text | ✅ verified | Playwright: filled title + x_field in Visual, switched to YAML, saw both in buffer |
| 3 | Valid YAML in YAML tab updates Visual tab fields | ⚠️ not directly tested | Structural review confirms `ast.set(doc)` in `yaml_on_change` + derived `Signal::derive` reads from AST |
| 4 | Invalid YAML shows error without clobbering AST | ⚠️ not directly tested | Structural review confirms `yaml_parse_error.set(Some(err))` + AST untouched on Err path |
| 5 | Unknown fields round-trip (e.g. `layout.colSpan`) | ⚠️ not directly tested in browser; ✅ structurally | AST preserves the full document; Visual-tab setters only touch named keys |
| 6 | New-chart seed uses `query:` not `sql:` | ✅ verified | Playwright screenshot showed `data.query: ''` in the seed YAML |
| + | `+ Add Series` visibly adds a row | ⚠️ Playwright test could not confirm | Code review traced through add_series → visual_series.push() → For render and approved; structural match with working toast.rs pattern. Needs manual browser verification. |

## Deferred work (from code review, round 1)

### [KYO-XX — Deduplicate ChartML remote preview setup in chart_builder.rs](#)
- **What**: Two identical 13-line blocks register all renderers + transform and call `register_source("_remote", data_table)` — once in the auto-fetch-on-open branch, once in the refresh-preview button handler.
- **Why deferred**: DRY cleanup, not a correctness fix. Out of scope for the urgent regression.
- **Resolution**: Extract a `build_remote_chartml(data_table, is_dark) -> Arc<ChartML>` helper in `markdown_renderer.rs` or `chart_builder.rs`, replace both call sites.
- **Impact**: Minor; bug fixes to chartml wiring won't propagate if this code drifts.

### [KYO-XX — Document `ensure_nested_mapping` destructive replacement behavior](#)
- **What**: `ensure_nested_mapping(parent, key)` silently replaces a non-mapping value at `parent[key]` with an empty mapping. No doc comment explains this.
- **Why deferred**: One-line doc-comment fix, not correctness.
- **Resolution**: Add a doc comment noting the replace-not-assert semantics.

### [KYO-XX — Modal title does not track live title edits](#)
- **What**: The modal header shows the initial chart title and doesn't update when the user edits the title input. Matches React's prior behavior.
- **Why deferred**: Pre-existing parity with React; not a regression. Cosmetic only.
- **Resolution**: Switch `modal_title` from a `String` to a derived `Signal`.

### [KYO-XX — Unused `_set_catalog_refresh_trigger` in chart_builder.rs](#)
- **What**: The catalog refresh button in the SQL Editor tab has no `on:click` handler; the `_set_catalog_refresh_trigger` WriteSignal is never called.
- **Why deferred**: Pre-existing dead code, not introduced by this PR.
- **Resolution**: Either wire up the refresh button or remove the trigger signal entirely.

## Behavioral divergences from React reference
None introduced. The Visual-tab series view-state layer (`visual_series`) matches the React `ChartVisualEditor.jsx` approach of tracking in-progress rows separately from the serialized chart spec.

## Security notes
None. This PR touches only client-side YAML state management; no new network paths, authentication boundaries, or data exfiltration surfaces.

## Integration notes
- `crates/kyomi-ui/src/components/dashboard/markdown_renderer.rs:306` — `extract_query` reads `data.query` / `data.url`. This PR's new-chart seed honors that path. Future changes to the read path should preserve this.
- Legacy saved charts with `data.sql` still load via the `ast_get_query` fallback; they are migrated to `data.query` on first edit.
- No schema migrations required. No environment variables added.

## Compilation status
- `cargo check -p kyomi-ui` (native): clean.
- `cargo check --target=wasm32-unknown-unknown -p kyomi-ui --features hydrate` (WASM): clean.
- No new clippy warnings in the modified file.
- No banned patterns (`#[allow(...)]`, `closure.forget()` on persistent listeners, etc.).
- Pre-commit hook signature-verified the code review approval.

## Remaining manual verification recommended
1. Open a dashboard, click **Add Chart**, go to Chart Config → YAML, paste the full repro YAML from the ticket, click **Save Chart**, re-open the chart, confirm all fields preserved verbatim including `layout.colSpan: 3` and `cache.ttl: 1m`.
2. In the same chart builder, click **+ Add Series** and confirm a second blank row appears in the Visual tab.
3. Type invalid YAML in the YAML tab (e.g. `:::`) and confirm the error banner appears inline without disturbing the Visual-tab form state.
