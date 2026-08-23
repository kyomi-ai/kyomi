# Anchor a source-text marker on code the regression must change — and fail as an assertion, not a panic

Leptos view trees can't be exercised as plain unit tests, so this codebase pins
their wiring with source-text guard tests: `include_str!` the file into `SRC`,
slice a window out with `extract_between(SRC, start, end)`, and assert against
what that window contains. The technique works — it has caught real regressions
— but it has a failure mode that reading the test cannot reveal, because both
markers are *leftmost-match, not unique-match* (`src.find(start)`, then
`src[start_pos..].find(end)`) and a missing marker is a `panic!`, not an
assertion failure.

That combination produces two recurring defects. First, a marker anchored on
something the regression itself rewrites — a line of user-visible copy, a doc
comment's own wording, the exact statement a wrap-mutation replaces — is deleted
by the very change the test exists to catch, so the test dies with
`end marker not found after "…"` and never reaches its assertion. It is still
red, so it is not worthless; but the failure names the marker instead of the
defect, and the next reader's cheapest reading of it is "someone renamed
something," which is exactly wrong. Second, a marker chosen because it "appears
only once" stops being unique the moment a test mentions the same string, and
the property that actually made the slice correct was that the real definition
comes *first*.

**Rule:** Anchor both markers on structure the regression cannot remove without
also failing the assertion — a `fn` signature, a match-arm head, a component or
prop name, `MOD_TESTS_MARKER` as an explicit end-of-production-code boundary.
Never anchor on UI copy, on comment prose, or on the specific statement the
regression rewrites. Then make the assertion, not the extraction, be what fails:
assert an occurrence count or a substring within the window, with a message that
names the defect and the ticket. When you mutation-test the guard (which
[prove-test-fails-without-fix.md](prove-test-fails-without-fix.md) requires),
check *how* it failed — a `marker not found` panic on the mutation means the
anchor is wrong, even though the run was red. And when justifying a marker,
state the leftmost-match property rather than a uniqueness tally; see
[name the invariant, not a count](../comments-documentation/name-the-invariant-not-a-count.md).

```rust
// WRONG — the end marker is the very line a regression rewrites, so the
// regression kills the extraction instead of the assertion
let others_arm = extract_between(
    SRC,
    "OAuthMessage::SnowflakeError(msg)",
    "set_error.set(Some(msg));",   // wrapping this call is the regression
);
assert!(!others_arm.contains("translate_google_oauth_error"));
// regression → panic: end marker not found after "OAuthMessage::SnowflakeError(msg)"

// RIGHT — anchored on structure the regression can't delete, and the count
// assertion is what fails, naming the defect
let arms = extract_between(SRC, "fn install_oauth_listener(", MOD_TESTS_MARKER);
let calls = arms.matches("translate_google_oauth_error(").count();
assert_eq!(
    calls, 1,
    "translate_google_oauth_error must be applied in the GoogleError arm only; \
     found {calls} call sites — another provider's OAuth failure is now being \
     rendered with Google-specific access-request copy (KYO-421)"
);
```

Real precedent, all four from the `2026-08-20` → `2026-08-24` review logs; cited
by log entry and symbol rather than file:line because most were logged against
in-flight branch state.

- **KYO-421 (`07:52` entry, 2026-08-24)** — 🟢. Both new `extract_between` guard
  tests, in `datasource_onboarding.rs` and on `translate_google_oauth_error`,
  fail via a `marker not found` panic on the exact regression they exist to
  catch. Confirmed by hand-mutating both the Google-arm-split and the
  others-arm-wrap regressions: the guards are real, just poorly diagnosed. The
  review's own suggested fixes are the two halves of the rule above — anchor the
  end marker on something structural, or assert a
  `translate_google_oauth_error(` occurrence count instead of extracting a
  substring.
- **KYO-407 (`00:49` entry, 2026-08-23)** — the same shape accepted as a
  mutation proof: renaming only the production-side marker comment, leaving the
  test's search string untouched, makes `assert_route_has_production_evidence`
  fail with "end marker not found". Red for the right reason, reported in the
  wrong words.
- **KYO-407 (`20:57` and `21:40` entries, 2026-08-21)** — cycle 1 flagged the
  `GenericTestAndDiscover` UI-copy end marker as a durability risk; the risk
  "did materialize within the hour via #370," and the repair was explicitly
  judged "a genuine improvement in anchor durability, not a relocation of the
  same fragility." Anchor choice, not assertion strength, was the whole
  difference.
- **KYO-429 (`05:06` entry, 2026-08-22)** — the leftmost-match half: verifying
  the guard test required extracting `DatasourcesPage`'s real body by hand,
  because the marker pair false-matched on `extract_between`'s *own* string
  literal arguments further down the file.

Separate from, and downstream of,
[one test topic per file](../code-organization/one-test-topic-per-file-not-one-big-mod-tests.md),
which covers the `SRC` self-inclusion compensation that the `tests/` split
removed. That rule is about what `SRC` contains; this one is about where inside
it you are allowed to point.
