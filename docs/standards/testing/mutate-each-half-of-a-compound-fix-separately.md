# Mutate each half of a compound fix separately, and name the assertion that died

[prove-test-fails-without-fix.md](prove-test-fails-without-fix.md) says: revert the fix,
watch the test go red, restore. That protocol has a silent gap when the fix has more than
one part. A single revert of *everything* the diff changed will turn the tests red — and
that red says only "at least one of these parts is guarded somewhere." It does not say
which part, and it does not say by which test.

The gap opens because tests written for a two-part fix usually share a setup assertion. A
test that checks `message_source` first asserts that exactly one row exists, because
reading `rows[0]` on an empty vec panics; a test that checks a log line first asserts the
row is absent, because that is the state the branch runs in. Revert both halves at once and
that shared, insensitive assertion is the one that fires. The test goes red, the report
says "mutation-verified", and the half the acceptance criterion actually names has no proof
of coverage at all — its own assertion was never reached.

The tell is that the assertion which failed is not the assertion whose name matches the
ticket. A red run whose failure text reads `sanity: exactly one user row` or `found 2 rows`
is evidence about persistence, not about the column, the log line, or whatever else the
same commit also changed.

**Rule:** When a fix changes more than one thing, mutate each part on its own, one at a
time, and record for each mutation *which* assertion failed and in *which* test. A
combined mutation is a starting point, not the evidence. If a part's own mutation leaves
every test green, that part is uncovered — say so. If a part's mutation turns a test red on
a shared setup or sanity assertion rather than on the assertion naming that part, the part
is still uncovered: the test rode the other half's guard.

```rust
// The fix changed two things: the persistence variant AND the message_source column.
// WRONG — one mutation, both halves reverted at once.
//   revert to: UserMessagePersistence::AdapterPersists(None) + message_source: None
//   → watch_turn_via_adapter_persists_exactly_one_user_message      FAILS ("found 2")
//   → watch_turn_via_adapter_persists_kyomi_watch_message_source    FAILS ("sanity: exactly
//                                                                    one user row")
//   Read as "both halves are guarded". But the second test died on the sanity assertion
//   that precedes its real one — the message_source assertion never ran.

// RIGHT — revert ONLY message_source, leave CallerPersisted intact.
//   → watch_turn_via_adapter_persists_exactly_one_user_message      still passes (correct:
//                                                                    it does not guard this)
//   → watch_turn_via_adapter_persists_kyomi_watch_message_source    FAILS on its
//                                                                    message_source assertion
//   Now the column half has its own independent, correctly-targeted guard.
assert_eq!(
    user_rows.len(), 1,
    "sanity: exactly one user row must exist before checking its message_source"
);
assert_eq!(user_rows[0].message_source.as_deref(), Some("Kyomi Watch"), /* ... */);
```

Two tickets, five review entries, one day:

- **KYO-573** (review log `2026-09-01`, cycles at `00:47` / `01:35` / `01:20`) — the fix set
  `UserMessagePersistence::CallerPersisted` *and* threaded `message_source` through
  `prepare_watch_dispatch` (`crates/kyomi-agent/src/watch_execution.rs`). Cycle 2 ran the
  combined mutation and got both `watch_turn_via_adapter_persists_*` tests red, then ran the
  narrower one cycle 1's report had suggested: *"reverted **only** `message_source` back to
  `None`, leaving `CallerPersisted` intact — `watch_turn_via_adapter_persists_exactly_one_user_message`
  still passes, and `watch_turn_via_adapter_persists_kyomi_watch_message_source` fails
  specifically on its `message_source` assertion (not the row-count sanity check), confirming
  that half of the fix has its own independent, correctly-targeted guard."* Under the combined
  mutation alone that second test had failed on `sanity: exactly one user row` — the shared
  assertion, not its own.
- **KYO-579** (review log `2026-09-01`, `20:50` cycle 2 and `21:00` cycle 3) — the same
  distinction, one cycle apart, on `save_agent_error` (`crates/kyomi-auth/src/chat_service.rs`).
  Cycle 2's test proved the fallback-insert branch was reachable via a real FK violation and
  asserted the row was absent; the reviewer reverted the fix to `let _ = add_message(...).await;`
  and the test *"passes identically pre- and post-fix"* — the row-absence assertion is
  insensitive to the change, because the FK violation fires either way. Cycle 3 added the
  assertion the acceptance criterion actually named (that the failure is logged) and the same
  mutation *"FAILED at the log assertion specifically — `captured: []` … which is the
  `error_events.is_empty()` assertion, not the row-count assertion above it (that one passed,
  as expected: the FK violation still fires with `let _ =`, it's just silently discarded)."*
  Naming which assertion died is what separated cycle 3's evidence from cycle 2's.

Nearest sibling is
[a-mutation-only-counts-if-the-run-could-have-failed.md](a-mutation-only-counts-if-the-run-could-have-failed.md),
whose red-side clause asks whether the failure text is *"your assertion's own message — not a
panic, not a harness guard, not a compile error."* That rule is about red for the wrong
*reason*: the run died before any assertion of yours was evaluated. This rule is about red at
the wrong *assertion*: the failure genuinely is one of your assertions, correctly evaluated,
just not the one covering the part you mutated — so the run is credible and still proves less
than it appears to. Distinct from
[cover-the-path-the-criterion-names-not-an-adjacent-one.md](cover-the-path-the-criterion-names-not-an-adjacent-one.md),
which is about testing an adjacent *layer* rather than the entry point the AC names; here the
layer is right and it is the assertion within it that is adjacent. Builds directly on
[prove-test-fails-without-fix.md](prove-test-fails-without-fix.md), which establishes the
mutate-and-restore protocol this rule sets the granularity for — and whose restore discipline
(`cp` from a backup, never `git stash`, per
[no-git-stash-copy-file-instead.md](no-git-stash-copy-file-instead.md)) applies once per
mutation, not once per review.
