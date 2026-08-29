# Assert the value the wiring computes, not that the wiring happened

A guard test that pins a call site — `block.contains("row_is_deleting(")`, an occurrence
count of `translate_google_oauth_error(`, "the derive delegates to
`connection_step_satisfied_from`" — answers one question: *is the right thing reached from
the right place?* It cannot answer the other one: *is it reached with the right value?* In
the diff those read as a single question, because the wiring and the binding sit on the same
line, and any mutation that severs the wiring turns the test red. So the guard
mutation-proves cleanly and looks load-bearing while covering half the defect surface.

The half it cannot see is the half that ships. The read stays, the call stays, and what gets
bound to it is wrong: a per-row predicate that drops its id comparison still reads both ids;
an error message that hardcodes its text still reads the map it was supposed to interpolate
from; two copies of one predicate, each spelled correctly at its own site, still answer
differently for the same input, and nothing fails when they drift. Every one of those stays
green under a wiring assertion, and the mutation that exposes it is a one-liner the author
would not think to try — because at the site the test points at, the code looks right.

**Rule:** For every behaviour a test claims to guard, assert the *answer* for concrete
inputs, including the input that separates the correct binding from the plausible wrong one
— a *different* row's id, an error reason that must appear in the rendered message, the mode
that must not be exempted. If the value only exists inside a reactive closure, extract the
decision into a pure function the production path calls and assert on that; keep the
source-text guard as a second layer proving the path still routes through it, never as the
only layer. Where two implementations must agree, assert that they agree for the same input
rather than that each looks correct in isolation.

```rust
// WRONG *as the only coverage* — this pins that the derive delegates and reads
// both signals. Collapsing row_is_deleting's body to `delete_pending` alone
// leaves all three assertions true, so this stays green while every row in the
// list spins during one row's delete.
let block = extract_between(
    SRC,
    "let is_deleting = Signal::derive(move || {",
    "let ds_for_toggle = ds.clone();",
);
assert!(block.contains("row_is_deleting("));
assert!(block.contains("datasource_to_delete.get()"));
assert!(block.contains("delete_pending.get()"));

// RIGHT — keep the wiring guard, and add the assertion that calls the real
// predicate with the input the two bindings disagree on: pending is true, but
// this is not the row being deleted.
assert!(!row_is_deleting(Some("ds-1"), "ds-2", true));
```

Precedent — one 🟡 that blocked signing, and four cases where the value assertion is what
caught the defect or was named as the reason the diff could be signed:

- **KYO-443 / KYO-426** (review log `2026-08-22`, `08:40` → `09:35` cycle 2) — 🟡, blocked
  signing. The create-mode/empty-slug guard existed twice: the pure
  `oauth_status_source_to_fetch` and an inlined copy in `use_oauth_status_refetch`'s `Memo`.
  The two replacement tests "pin today's source text/branch order but do not assert the
  Memo's decision matches `oauth_status_source_to_fetch`'s for the same inputs — nothing
  fails if the two are edited to diverge." Cycle 2 gave the predicate a lazy accessor
  (`read_slug: impl FnOnce() -> String`) so the `Memo` delegates to the one implementation,
  and replaced the source-text pins with tests passing a `read_slug` closure that panics if
  invoked — "a genuine regression trap, not source-text pinning."
- **KYO-467** (`2026-08-23`, `19:15`) — the reviewer's own mutation collapsed
  `row_is_deleting` to `delete_pending` alone. Exactly three tests failed, all of them value
  assertions; the wiring test above stayed green. Logged as "the KYO-477 lens (test proves
  the signal is *computed* correctly, not merely *read*)."
- **KYO-466 cycle 2** (`2026-08-23`, `17:45`) — mutation kept the
  `resource_errors.get("projects")` read and hardcoded the message instead of interpolating
  the bound reason; `test_action_effect_computes_bq_projects_error_from_the_bound_reason`
  failed. Named in the log as "exactly the KYO-427 failure mode (right plumbing, wrong bound
  value)."
- **KYO-411** (`2026-08-21`, `19:49`) — a test that string-matched a `Signal::derive` body
  was rewritten to call `connection_step_satisfied_from` directly across three input
  combinations. Judged "strictly stronger than the original — string presence can't verify
  logical wiring, direct calls can," and explicitly not test-weakening: the extraction is
  what made the stronger assertion possible.
- **KYO-427** (`2026-08-23`, `12:26`) and **KYO-406** (`2026-08-24`, `15:40`) — both signed
  with the distinction stated in the notes. KYO-427's new tests are "reachability tests
  (mutation-provable) rather than the source-marker truth-table style that can pass while
  behavior is wrong"; KYO-406's asserts "on the actual computed `Vec` a real `Select` would
  receive, not just that a constant string appears in the source."

Complements
[anchor a source-text marker on code the regression must change](anchor-source-text-markers-on-code-not-copy.md),
which makes a wiring assertion fail for the right reason — this rule is about the defect a
wiring assertion cannot reach at all — and
[cover the path the acceptance criterion names](cover-the-path-the-criterion-names-not-an-adjacent-one.md),
the same instinct on a different axis: that rule asks *which entry point*, this one asks
*what the assertion looks at* once you are there. The mutation that tells them apart is the
one [prove the test fails without the fix](prove-test-fails-without-fix.md) already requires
— break the binding without touching the wiring, and see whether anything goes red.
