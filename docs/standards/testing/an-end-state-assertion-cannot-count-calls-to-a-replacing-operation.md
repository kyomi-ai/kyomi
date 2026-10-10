# An end-state assertion cannot count calls to an operation that replaces its own output

Some operations are written to *replace* what they produced last time, not add to it: a
rechunk that deletes a document's existing chunks before inserting new ones, an upsert keyed
on a natural id, a cache fill that overwrites its slot, a file write that truncates first.
That design is deliberate — it is what makes the operation safe to retry. It also means that
running it once and running it twice with the same input leave **identical** state behind.

A test that wants to prove "this path runs the operation exactly once" is tempted to read
that state back and count it: one row, not two, so it ran once. The count cannot say that.
Double invocation converges on the same single row, so the assertion is green for the
correct code and for the regression it names. It is not flaky and not wrong about the
state — it simply answers a different question from the one its comment claims. And
because it is attached to a confident comment ("a count of 2 would mean it ran twice"),
the next reader trusts it as the guard and nobody writes the real one.

This is distinct from the fixture problem in
[choose-inputs-the-broken-code-would-answer-differently.md](choose-inputs-the-broken-code-would-answer-differently.md):
no choice of input fixes it, because the operation's *own contract* erases the evidence of
the second call. It is distinct from
[an-assertion-a-constant-already-satisfies-cannot-fail.md](an-assertion-a-constant-already-satisfies-cannot-fail.md)
too: the value is genuinely computed by the run — it is just invariant under the defect.

**Rule:** Before asserting on state to prove how many times something ran, read the
operation's body and ask whether a second call with the same input would change that state.
If it would not (delete-then-insert, upsert, overwrite, truncate-then-write), the state
cannot carry a call-count claim. Then either:

- **observe the calls, not the state** — count them at a seam that records invocations, or
  vary the input between the two would-be calls so a second one leaves a distinguishable
  trace; or
- **move the guarantee somewhere structural** — a single call site that every path is
  forced through — and say in the test's doc comment that the test does *not* prove the
  count and where the guarantee comes from instead.

What you must not do is keep the count assertion and its comment. A test may prove less than
you hoped; it may not claim more than it proves.

```rust
// WRONG — a reconstruction, not a quote: the cycle-1 form was corrected in review
// before the branch's only pushed commit, so no `<sha>` carries it; this is the
// shape the review log records.
// `rechunk_document` deletes the document's existing knowledge_chunks rows before
// inserting, so a double rechunk on create also leaves exactly one row.
assert_eq!(
    chunk_contents.len(),
    1,
    "a count of 2 would mean the create path rechunked twice"
);

// RIGHT — the test asserts only what the state can show, and its doc comment
// (at commit 5e658755, in flight on PR #546) says where the once-only
// guarantee actually lives:
//
//   /// This does NOT prove "only one rechunk happened" — `rechunk_document`
//   /// (`crates/kyomi-auth/src/dashboard_service.rs`) deletes existing
//   /// chunks before inserting, so two sequential, awaited calls with the
//   /// same content converge on the same single row; a row-count assertion
//   /// cannot distinguish one call from two. That guarantee instead comes
//   /// structurally, from `apply_create` being the single call site both
//   /// `CreateDashboardTool` and this branch now go through — see its doc
//   /// comment in `tools/document/mod.rs`.
assert_eq!(
    chunk_contents,
    vec!["Runbook content, single chunk.".to_string()],
    "write_knowledge_file must populate knowledge_chunks with the created content \
     before any edit: {chunk_contents:?}"
);
```

Real precedent:

- **KYO-776, the `create-path rechunk choke point + mined standards` review**
  (`2026-09-23` log) — 🟡, *Test Manipulation (overclaim)*, on
  `write_knowledge_file_populates_knowledge_chunks_on_create` in
  `crates/kyomi-agent/src/tools/knowledge.rs`. The reviewer read
  `dashboard_service::rechunk_document`, confirmed it deletes existing `knowledge_chunks`
  rows before inserting, and recorded that *"sequential double-invocation converges to the
  same single-row final state. The row-count assertion structurally cannot detect the
  regression it claims to guard against; the comment overstates what the test proves."*
  The cycle-2 re-review signed after the count assertion was removed and the doc comment
  rewritten to attribute the once-only guarantee to `apply_create`'s single call site —
  the second remedy above. At the time of writing that change is in flight on PR #546
  (commit `5e658755`), not yet on `main`.

See also [prove-test-fails-without-fix.md](prove-test-fails-without-fix.md): the mutation
that exposes this is "call the operation twice", and it stays green — which is the finding.
