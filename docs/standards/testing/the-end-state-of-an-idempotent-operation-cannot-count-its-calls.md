# The end state of an idempotent operation cannot count its calls

A regression of the form "this now runs twice" is a claim about *how many times* something
happened. Reading back state afterwards answers a different question — *what is there now* —
and for an operation that replaces rather than accumulates, those two questions have the same
answer for one call and for ten. A test that seeds, runs, and asserts `rows.len() == 1` looks
like it pins "exactly once". It pins nothing about the count: the operation converges, so the
double-call regression leaves the table byte-identical and the assertion green.

The trap is that the assertion *would* catch the regression against a naive implementation —
a plain `INSERT` duplicates rows when called twice — so it reads as obviously load-bearing.
Whether it is depends on the body of the helper under test, not on the test. In this codebase
the helper in both precedents is `dashboard_service::rechunk_document`
(`crates/kyomi-auth/src/dashboard_service.rs`), which deletes the document's existing
`knowledge_chunks` rows before inserting the new ones inside one transaction. Two sequential,
awaited calls with the same content therefore end in exactly the state one call does.

The same shape applies to any upsert, `DELETE`-then-`INSERT`, `set` of a signal to the same
value, overwrite of a file, or cache `put` of the same key: its end state encodes the *last*
call, never the number of calls.

**Rule:** Before asserting on end state as evidence that something ran once (or did not run
twice), read the operation's body and ask whether a second call with the same input would
change anything you are about to read back. If it would not:

- make the count observable through something that does *not* converge — a per-call counter
  or spy on the real seam, a tracing event captured under a registered subscriber (see
  [a-tracing-assertion-needs-the-callsite-registered-under-a-subscriber.md](a-tracing-assertion-needs-the-callsite-registered-under-a-subscriber.md)),
  or a per-call artefact such as a freshly generated row id captured between the calls; or
- enforce "once" structurally — a single choke-point call site — and have the test assert
  only what state inspection can prove (the content is correct and present), with a comment
  that names where the single-call guarantee actually comes from and why this test does not
  carry it.

Never keep a count assertion whose comment claims it would catch a double call, unless you
have run that double call and watched the assertion go red
([prove-test-fails-without-fix.md](prove-test-fails-without-fix.md)).

```rust
// WRONG — the shape flagged in the KYO-776 review (cycle 1, finding #2). It was
// removed before landing and never committed, so there is no `<sha>^` to quote;
// this reproduces the shape the review log describes, not verbatim text.
assert_eq!(
    chunk_contents.len(),
    1,
    // "a count of 2 would mean the create path rechunked twice"
);
// rechunk_document deletes before it inserts: a second call leaves exactly one
// row, so this line is green whether the create path rechunks once or twice.

// RIGHT — the KYO-541 test's doc comment in `crates/kyomi-agent/src/tools/knowledge.rs`,
// as it stands on `main` (excerpt), above
// `edit_knowledge_file_refreshes_knowledge_chunks_via_unified_path`:
/// Single-dispatch — deliberately NOT claimed in this test's name,
/// because nothing here measures it — is enforced by
/// `apply_update` passing `embed: None` to
/// `dashboard_service::update_dashboard`, ...
/// confirmed by reading that both call sites remain as described, not by a
/// runtime assertion here: `rechunk_document` deletes its document's chunks
/// before re-inserting, so a second sequential call converges to
/// identical rows rather than duplicating them, ...
/// What this test asserts is the one thing state-inspection
/// after `execute()` returns actually can prove: the refreshed content
/// is correct and present.
```

Real precedent — the same helper, two tickets, one week apart:

- **KYO-541** — `docs/review-logs/2026-09-14.md`, *"KYO-541: refresh both dashboard/knowledge
  search indexes on edit"*, 17:16 entry, finding #2 (🟢, Test Coverage — overclaim): the test
  `edit_knowledge_file_refreshes_knowledge_chunks_exactly_once` "implies a runtime-verified
  'exactly once' guarantee; the body only asserts final content correctness". Resolved in the
  re-review by renaming it and rewording the doc comment quoted above.
- **KYO-776** — `docs/review-logs/2026-09-23.md`, *"KYO-776/KYO-798: create-path rechunk choke
  point + mined standards"*, 16:20 entry, finding #2 (🟡): a test comment claimed "a count of 2
  would mean the create path rechunked twice", but `rechunk_document` "deletes existing
  `knowledge_chunks` rows before inserting new ones — sequential double-invocation converges to
  the same single-row final state. The row-count assertion structurally cannot detect the
  regression it claims to guard against." Resolved in cycle 2 (16:45 entry) by removing the
  count assertion and attributing the no-double-rechunk guarantee to the single call site.
  The fix was still on an open PR when this rule was written, so it is cited by review-log
  entry only.

Nearest siblings, and how this one differs:

- [an-assertion-a-constant-already-satisfies-cannot-fail.md](an-assertion-a-constant-already-satisfies-cannot-fail.md)
  — there the assertion's *predicate* is satisfied by a literal in the production code. Here
  the predicate is sound and the value is really computed; it is the *operation's semantics*
  that make the regressed and correct runs produce the same value.
- [choose-inputs-the-broken-code-would-answer-differently.md](choose-inputs-the-broken-code-would-answer-differently.md)
  — there a better fixture fixes it. Here no fixture can: with any input, one call and two
  calls converge. The remedy is a different observable or a structural guarantee.
- The naming side of the same KYO-541 finding — a test *name* still claiming "exactly once"
  after the body stopped measuring it — is a separate lesson about names as claims; this rule
  is about the assertion itself.
