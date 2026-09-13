# Vary a fixture only on the axis under test — anything else decides the outcome first

A regression test seeds some fixtures, runs the real code path, and asserts an outcome. The
property under test is one dimension of those fixtures: which of two textual timestamp formats
a value is rendered in, whether a second sign-in method exists on the account. If the fixtures
*also* differ from the subject along some other dimension — a different calendar month, a
missing row an earlier guard checks — that other dimension decides the outcome before the
tested one is ever consulted. The test then passes against the shipped bug and against its fix
alike, and it does so while looking like exactly the coverage the ticket asked for.

[prove-test-fails-without-fix.md](prove-test-fails-without-fix.md) is the check that exposes
this: revert the fix, watch the test stay green. But a green mutation only tells you the test
is vacuous; it does not tell you what to change, and the instinct — strengthen the assertion —
is often the wrong half. In both incidents below the assertion was already looking at the right
value. The fixtures were what made that value the same either way.

**Rule:** Build the comparison fixtures from the value the production path actually produces,
then vary exactly the dimension the test names and nothing else. If the code under test is
a guard or a branch, seed everything the *earlier* guards need so the call genuinely reaches
the branch, and assert something only that branch can produce — the mock server received the
request, the specific error variant, the relative order of the items that a wrong comparator
inverts. When a fixture must be older/newer/different than a value the code generates at run
time, derive it from that value rather than hand-writing a literal beside it.

```rust
// WRONG — a reconstruction of the pre-fix shape, not a quote: the vacuous form was
// corrected during review and never committed. `new_item.updated_at` is the wire
// session's real `Utc::now()`, so it lands in the current month; every hand-written
// fixture is in an earlier one. The month digits differ before the `T`-vs-space
// separator is ever reached, so a byte-wise `Reverse(s.updated_at.clone())` — the
// exact KYO-490 defect this test exists to pin — still ranks the new session first.
store.upsert_chat_session(make_older_session("rfc3339-oldest", "2026-08-01T09:00:00+00:00"));
store.upsert_chat_session(make_older_session("postgres-older", "2026-08-02 09:00:00+00"));
store.upsert_chat_session(new_item);
sort_sessions_by_recency(&mut sessions);
assert_eq!(sessions.first().map(|s| s.session_id.as_str()), Some(client_sid.as_str()));

// RIGHT — quoted from `new_session_live_insert_tests.rs`, landed on `main` in
// `9acc7a87` (PR #520, squash-merge of `origin/jason/kyo-498-chat-session-live-insert`
// at `eb39e875`), with the two
// fixtures that are not load-bearing here and the assertion messages elided
// (`// ...` and `/* ... */` mark every cut). Every fixture is
// derived from the wire session's own `updated_at`, on its own calendar day, so the
// format separator is the only thing that can differ first — and the second
// assertion pins the order a byte-wise compare inverts.
let now = crate::utils::time::parse_timestamp(&new_item.updated_at).expect(/* ... */);
// ... `midnight` is local midnight on `now`'s own day, `elapsed` is `now - midnight`,
// and `rfc3339`/`postgres` are the two rendering closures.
let at = |numerator: i32, denominator: i32| midnight + elapsed * numerator / denominator;
store.upsert_chat_session(make_older_session("rfc3339-recent", &rfc3339(at(6, 8))));
store.upsert_chat_session(make_older_session("postgres-recent", &postgres(at(7, 8))));
// ...
let older_ids: Vec<&str> = sessions[1..].iter().map(|s| s.session_id.as_str()).collect();
assert_eq!(
    older_ids,
    vec!["postgres-recent", "rfc3339-recent", "postgres-older", "rfc3339-oldest"],
);
```

Real precedent — two reviews, two different extra dimensions, the same vacuity:

- **KYO-498** — review log `2026-09-12`, heading *"KYO-498: chat session live-insert regression
  coverage"*, one 🔴 that blocked signing, resolved in the *"(re-review, cycle 2)"* entry. The
  reviewer reverted `sort_sessions_by_recency` (`crates/kyomi-ui/src/pages/chat/chat_list.rs`)
  to a raw byte-wise `Reverse(s.updated_at.clone())` — the pre-KYO-490 bug — and
  `new_session_insert_snapshot_sorts_to_top_of_a_populated_store` still passed. The fix derives
  every older fixture from the wire session's own `updated_at`, anchored to the same calendar
  day, rendered half as RFC 3339 and half in the Postgres `CAST(... AS TEXT)` form, with
  `postgres-recent` deliberately placed *more* recent than an RFC-3339 fixture — the one pairing
  a byte-wise compare inverts, since `T` (`0x54`) outranks space (`0x20`) before either string's
  clock digits are compared. Under the same mutation the rewritten test now fails, and it fails
  on the added `older_ids` assertion; `sessions.first()` alone still reports the new session
  first. Landed on `main` in `9acc7a87` (PR #520), the squash-merge of branch
  `origin/jason/kyo-498-chat-session-live-insert` (`eb39e875`, single commit) — unmerged at
  authoring time, merged by the time this rule was rescued.
- **KYO-701** — landed in `30b21c88` (PR #507), review log `2026-09-10`, heading *"KYO-701:
  last-auth-method guard on Google disconnect"*. The reviewer found the *pre-existing*
  `disconnect_preserves_local_state_when_revocation_fails`
  (`crates/kyomi-auth/src/google_oauth.rs`) "passing for the wrong reason": it seeded only a
  `google_oauth` auth method and asserted only `result.is_err()`, so the diff's brand-new
  last-sign-in-method guard returned `Conflict` and satisfied that assertion without the
  revocation request ever being attempted. The fixture now also seeds a `password` method — so
  the earlier guard is not what fails — and the test asserts `requests.len()` against the mock
  server, which only the revocation path can produce. Both halves were needed: the extra
  fixture to reach the branch, the extra assertion to prove it was reached.

Sibling of [assert-the-value-not-that-the-call-happened.md](assert-the-value-not-that-the-call-happened.md):
the same vacuity arriving through the assertion instead of the fixture. KYO-716 (review log
`2026-09-12`, heading *"KYO-716: migration schema drift detection"*, a 🟡 fixed in the
re-review) is that shape, not this one: `assert!(status == "healthy" || status == "degraded")`
in `apps/server/tests/contract_health.rs` enumerated every value the field can hold by
construction, so it could not fail under a mutation that flipped `status` to `"degraded"` on
drift. It now reads `assert_eq!(status, "healthy", "drift must not change status")`. Counted
there rather than here, per
[count-only-the-incidents-that-instantiate-the-rule.md](../comments-documentation/count-only-the-incidents-that-instantiate-the-rule.md).

Distinct from [a-mutation-only-counts-if-the-run-could-have-failed.md](a-mutation-only-counts-if-the-run-could-have-failed.md):
there the run never compiled the mutated code, never reached the assertion, or went red for an
unrelated reason, so the observation is about the harness. Here the mutation compiled, the test
ran, the assertion executed — and was satisfied anyway. The remedy is in the fixture data, not
in the invocation.

Distinct from [a-tests-verdict-must-not-depend-on-the-ambient-environment.md](a-tests-verdict-must-not-depend-on-the-ambient-environment.md):
that rule keeps an *uncontrolled* input out of the assertion. KYO-498's `Utc::now()` is not
uncontrolled — it is the value under test, arriving through the real dispatch path, which is
precisely why the fixtures must be derived from it rather than written beside it. The KYO-498
re-review did record one residual timing edge in the derived form (whole-second Postgres-format
fixtures could collide within ~1.6s of UTC midnight, judged non-blocking); if a derivation
cannot be made total, [nondeterministic-verdict-is-a-failing-test.md](nondeterministic-verdict-is-a-failing-test.md)
governs what to do about it.
