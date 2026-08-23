# Anchor a citation to a symbol, not to a line number

A `file:line` pointer in a comment, a standards rule, or a test's doc block is correct
for exactly as long as nobody inserts a line above it. In `datasources.rs` that is
hours. The pointer does not rot loudly — it silently retargets, so a reader who follows
it lands on unrelated code and concludes the *claim* is wrong rather than the *anchor*.
Both failure modes cost review cycles: a citation that resolves to something plausible
but different is worse than no citation, and "just update the numbers" reproduces the
defect on the next merge.

Line numbers are also uniquely hostile to this repo's workflow. Several agents work the
same file concurrently, tests get appended to a shared `mod tests` tail, rebases shift
everything below a hunk, and the reviewer reading your citation is usually on a
different commit than the one you wrote it on.

**Rule:** cite the thing that has a name — the function, component, signal, test, marker
string, or ticket. `crates/…/datasources.rs::BigQueryAuthModeSection`'s Remove chip is
findable forever; `datasources.rs:5059-5074` is findable until the next PR. If you need
a line number to describe a diff or a historical finding, say what commit or date it was
true at, and make the sentence still useful once it drifts. When a reviewer flags a stale
pointer, delete the pointer rather than re-pointing it — repointing addresses the symptom
and guarantees a repeat.

```rust
// WRONG — two navigational pointers, already off by 77 and 345 lines when
// a reviewer checked them one day later
/// Evidence for this route: the generic Test & Discover `<Show>` gate
/// (~line 3763) and the BigQuery service_account Validate button (~line 4959).

// RIGHT — names the enclosing component and the wiring, which survive edits
/// Evidence for this route: the generic Test & Discover `<Show>` gate rendered
/// inside `DatasourceModal`, and the "Validate & Discover Projects" button
/// rendered inside `BigQueryAuthModeSection`, wired through `on_validate`.
```

Three reviews in two days, each on a different ticket:

- **KYO-407 (2026-08-22 review log, `20:47` initial and `16:52` cycle 2)** — 🟢. Two
  `~line` pointers in a new test's doc comment had drifted by 77 and 345 lines by the
  time the reviewer checked them, on a file the same batch of PRs was actively editing.
  Fixed by removing the numbers and naming the enclosing components instead; the log
  records the reasoning explicitly — the fix "addressed the root cause (pointers that
  will always drift) rather than patching the symptom (updating two numbers that will
  drift again)."
- **KYO-429 (2026-08-22, `05:06` and `05:45`)** — 🟡, blocked signing twice. A new
  standards paragraph cited three `file:line` locations; none resolved. One pointed at
  an unrelated `google_disconnect_action` test, another at the Gmail dark-mode
  `background-color` section of a document. The eventual fix cited by review-log date
  and by the *shape* of each claim, with the rationale stated in the rule itself —
  the same convention [name-the-invariant-not-a-count.md](name-the-invariant-not-a-count.md)
  and [no-user-facing-claim-the-branch-doesnt-establish.md](../error-handling/no-user-facing-claim-the-branch-doesnt-establish.md)
  now follow.
- **KYO-433 (2026-08-23, `12:34` cycle 3)** — no finding, but only just. The reviewer
  started to flag `teardown-clears-the-whole-derived-state-group.md`'s KYO-413 citations
  (`datasources.rs:5059-5074`, `:2423`/`:2459`, `:4809`) as wrong against current source,
  then found the paragraph framed them as "Flagged in KYO-413 (2026-08-21)" — a quotation
  of the historical record, not a live pointer. The explicit dating is what saved it.
  Those same anchors have since drifted again: `google_disconnect_action` and
  `datasource_disconnect_action` now sit ~216 lines below the cited `:2423`/`:2459`.

Sibling of [verify every identifier in a doc code example](verify-every-identifier-in-a-doc-code-example.md):
that rule is about a citation that was never right; this one is about a citation that
was right and could not stay that way.
