# A new source-level test goes in its topic file, not one shared `mod tests`

A single `mod tests { ... }` block appended to the bottom of a large source file
(the pattern `datasources.rs` used before KYO-455) becomes a serialising resource
once more than one PR is adding tests to that area at a time: every test, on
every subject, appends near the same closing `}` at the tail of the file, so any
two concurrently-open PRs collide there — regardless of whether their tests have
anything to do with each other. Worse, the collision is silently destructive:
every test in the block ends `    }\n}`, so a three-way merge treats that suffix
as common context and hoists it once. Resolving the conflict the "obvious" way
then leaves the file missing a brace, which reads as clean, correctly-indented
Rust and deletes a pre-existing, unrelated test — catchable only by watching the
deletion count in `git diff --numstat`.

**Rule:** For a source file large enough to carry its own inline tests, split
`mod tests` into `<file_stem>/tests/mod.rs` plus one file per test *topic* —
grouped by subject matter, not by ticket number — so two PRs adding tests on
different topics touch different files and never collide. Put shared test
infrastructure (source-text-matching helpers, `include_str!` constants, marker
constants, assertion helpers) in `tests/mod.rs`; topic files pull them in via
`use super::{...};` and, for anything defined in the parent production module,
`use super::super::{...};`. When adding a test, put it in the topic file it
obviously belongs with; only add a new topic file when none of the existing
ones fit, and only fall back to a single flat `mod tests { ... }` for a file
too small to be a realistic collision target in the first place.

```
// WRONG — every regression test for every topic appends near the same tail
crates/kyomi-ui/src/pages/settings/datasources.rs   (11,715 lines; mod tests
                                                      alone is ~2,800 lines)

// RIGHT — one file per topic; a new OAuth test never touches catalog.rs
crates/kyomi-ui/src/pages/settings/datasources.rs        (production code only)
crates/kyomi-ui/src/pages/settings/datasources/tests/mod.rs        (shared helpers)
crates/kyomi-ui/src/pages/settings/datasources/tests/oauth.rs
crates/kyomi-ui/src/pages/settings/datasources/tests/create_mode.rs
crates/kyomi-ui/src/pages/settings/datasources/tests/catalog.rs
crates/kyomi-ui/src/pages/settings/datasources/tests/...
```

If the split module's tests assert against the source text of the file they
cover (`include_str!` plus string-matching, the pattern this codebase uses for
Leptos view trees that can't be exercised as plain unit tests), watch for a
second, easy-to-miss failure mode: before the split, that `include_str!`
necessarily pulled in the test module's own source alongside production code,
so any whole-file text search needed a compensating scope (slicing at a marker
that opened the test module, or bounding to a region known to sit above it) to
avoid matching its own literals. After the split, the constant contains
production code only — remove that compensation deliberately, one assertion at
a time, and confirm what each assertion means before and after; do not just
adjust numbers until the suite goes green again.

Flagged in KYO-375 (the same collision pattern in `docs/standards/` itself,
resolved by the one-rule-per-file layout this document follows) and KYO-455
(`datasources.rs`'s `mod tests`, which had cost rework on PRs #371, #379, and
three same-day rebases — #389, #391, and one more — before the split).
