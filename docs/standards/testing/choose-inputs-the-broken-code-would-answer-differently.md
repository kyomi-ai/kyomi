# Choose inputs the broken code would answer differently — a fixture that agrees by accident proves nothing

A test can compile under the right target, run, reach its assertion, and go green while the
defect it exists to catch is fully present — because the **fixture data** happens to produce
the right answer for a second reason. Nothing about the run looks wrong. The mutation is
real, the test name is on the line, the output says `ok`, and the honest reading of that is
"the assertion does not cover this". The reading that actually gets written down is "covered,
mutation-proven".

This is the input-side member of the vacuous-test family, and it is the one that survives
every other check in this section. The entry point is right. The assertion looks at the
value, not at the wiring. The run compiled the changed line. What went wrong is upstream of
all three: the fixtures differ from each other along *more* axes than the one under test, so
a cruder rule than the one being asserted already sorts, ranks or classifies them correctly.

Two shapes have appeared:

- **The fixtures differ on a coarser axis than the one under test.** A comparison is meant to
  prove that two textual timestamp formats are normalised before they are ordered, but the
  fixtures also differ by calendar month — and the month digit decides a raw byte compare
  before the format separator is ever reached. The axis under test is never exercised.
- **The fixture omits the precondition that separates two causes of one observable.** A test
  asserts `result.is_err()` on a path that can now fail for two different reasons, but seeds
  only the state that triggers the *other* one. The assertion passes whether or not the
  behaviour under test works, and it did so before the behaviour existed.

**Rule:** For each behaviour a test guards, name the axis the code is supposed to key on, then
build the fixtures so they are identical on every *other* axis the code could key on instead
— derive them from the same reference value where you can, rather than hand-writing
independent literals. Pick the specific pair the broken code gets backwards, and assert the
relationship *among the fixtures that carry the variation*, not only the position of the item
you injected. Where the observable is a failure, seed the state in which only the cause under
test can produce it. Then mutate: if the mutation is green, the fixture is the first thing to
suspect, not the last.

```rust
// WRONG — a reconstruction, not a quote. The pre-fix form was corrected during
// review and PR #520 squash-merged, so there is no `<sha>^` that carries it;
// this is the shape the review log records. Hand-written `2026-08` literals
// against a wire `updated_at` that is `Utc::now()`, and one assertion on the
// injected item alone.
store.upsert_chat_session(make_older_session("rfc3339-oldest",  "2026-08-01T09:00:00+00:00"));
store.upsert_chat_session(make_older_session("postgres-older",  "2026-08-02 09:00:00+00"));
store.upsert_chat_session(make_older_session("rfc3339-recent",  "2026-08-03T09:00:00+00:00"));
store.upsert_chat_session(make_older_session("postgres-recent", "2026-08-04 09:00:00+00"));
store.upsert_chat_session(new_item);        // updated_at is Utc::now(), i.e. 2026-09

sort_sessions_by_recency(&mut sessions);
assert_eq!(sessions.first().map(|s| s.session_id.as_str()), Some(client_sid.as_str()));
// Still green after reverting sort_sessions_by_recency to a raw byte-wise
// `Reverse(s.updated_at.clone())` — the exact defect this test documents.
// `2026-09` outranks every `2026-08` on the month digit, so the separator byte
// the defect turns on is never compared, and nothing pins the order among the
// four fixtures that carry the format variation.

// RIGHT — `crates/kyomi-ui/src/server_fns/chat/new_session_live_insert_tests.rs`
// on `main`, verbatim (the `at`/`rfc3339`/`postgres` helper definitions above
// these lines are elided). Every fixture is derived from the wire session's own
// `updated_at`, so the calendar date is shared and the separator is the only
// thing that can differ first.
store.upsert_chat_session(make_older_session("rfc3339-oldest", &rfc3339(at(1, 8))));
store.upsert_chat_session(make_older_session("postgres-older", &postgres(at(2, 8))));
store.upsert_chat_session(make_older_session("rfc3339-recent", &rfc3339(at(6, 8))));
store.upsert_chat_session(make_older_session("postgres-recent", &postgres(at(7, 8))));

let older_ids: Vec<&str> = sessions[1..].iter().map(|s| s.session_id.as_str()).collect();
assert_eq!(
    older_ids,
    vec!["postgres-recent", "rfc3339-recent", "postgres-older", "rfc3339-oldest"],
    "the four older, mixed-format sessions must also sort in real chronological order among \
     themselves — a byte-wise compare inverts postgres-recent/rfc3339-recent specifically, \
     since they share a calendar date and `T` (0x54) outranks space (0x20) before either \
     string's clock digits are ever compared: {sessions:?}"
);
```

Real precedent — two tickets, one 🔴 that blocked signing:

- **KYO-498, the `chat session live-insert regression coverage` review and its cycle-2
  re-review** (`2026-09-12` log) — 🔴, *Test Manipulation / False Success Claim*. The reviewer
  reverted `sort_sessions_by_recency` to `Reverse(s.updated_at.clone())` and **both tests
  still passed**: *"the month digit alone decides `sessions.first()` under a naive byte
  compare, so the mixed RFC3339-vs-Postgres-text format defect this test exists to catch is
  never actually exercised. The test asserts only `sessions.first()`, never the relative
  order among the four older, differently-formatted fixtures."* The entry records that this
  *"directly contradicts the 'mutation-proven' claim relayed for this test"* — the mutation
  had been run; it was the fixtures that made its verdict meaningless. Cycle 2 fixed both
  halves at once: fixtures re-derived from the wire timestamp so they share a calendar day,
  and a second assertion over the four fixtures' order among themselves. The reviewer re-ran
  the same mutation and confirmed the new assertion is the one that goes red.
- **KYO-701, the `last-auth-method guard on Google disconnect` review** (`2026-09-10` log) —
  the second shape, found while checking an untouched pre-existing test. The reviewer
  *"confirmed the pre-existing `disconnect_preserves_local_state_when_revocation_fails` test
  was passing for the wrong reason pre-fix: it seeded only `google_oauth` (no password) and
  asserted only `result.is_err()`, which the new guard's `Conflict` also satisfies"*. With
  only one auth method seeded, the new last-method guard fires before the revocation path is
  reached, so the assertion could not distinguish the two failures. The fix is the shape this
  rule asks for — *"added password fixture + `requests.len() == 1` assertion"* — seeding the
  state in which only the cause under test can produce the observable, and asserting on the
  side effect that separates them.

Sibling of
[cover-the-path-the-criterion-names-not-an-adjacent-one.md](cover-the-path-the-criterion-names-not-an-adjacent-one.md)
and
[assert-the-value-not-that-the-call-happened.md](assert-the-value-not-that-the-call-happened.md):
the three are the same instinct on the three parts of a test. That rule asks *which entry
point* you drive; the other asks *what the assertion looks at* once you are there; this one
asks *which inputs you hand it* — because a correct entry point and a correct assertion still
prove nothing about an axis the data never varies along on its own.

Distinct from
[a-mutation-only-counts-if-the-run-could-have-failed.md](a-mutation-only-counts-if-the-run-could-have-failed.md):
there the run itself was incapable of the outcome — the mutated body was never compiled by
that target, cargo replayed a stale fingerprint, the mutation hit a different copy of the
script. Here the run is entirely capable: the mutated code compiled, executed, and produced
the assertion's value. The fixture simply made the correct and the broken answers the same,
which no check on the run can see.

Distinct from
[assert-result-size-not-just-seeded-rows.md](assert-result-size-not-just-seeded-rows.md): that
rule is about the assertion being too *narrow* — reading back only your own rows cannot see
what else the query returned, and the remedy is to assert the result's size. Here the
assertion's scope is not the problem; widening it would not help while the injected item wins
on the month digit. The remedy is to change the inputs so the comparison under test is the
one that decides the answer.

See also [prove-test-fails-without-fix.md](prove-test-fails-without-fix.md): the mutation it
requires is how this defect is found, and KYO-498 is the case where that mutation was
performed, reported, and still left the gap — a green mutation is a finding about the test,
and the fixture is where to look first.
