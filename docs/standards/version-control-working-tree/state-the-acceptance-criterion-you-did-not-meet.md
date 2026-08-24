# State the acceptance criterion you did not meet

A diff can be entirely correct — every line does what it claims, every test passes, clippy is
clean — and still leave a ticket's own acceptance criterion completely untouched. Nothing in
`cargo check`, `cargo test`, or a line-by-line read of the diff will surface that: the gap
only exists relative to a document the diff itself never mentions. The one way to catch it is
to re-read the ticket against the diff and ask, AC by AC, "did this land?" — an audit nobody
performs by default, because a clean diff *looks* like a finished ticket from the inside.

This is not the test-coverage case in
[cover-the-path-the-criterion-names-not-an-adjacent-one.md](../testing/cover-the-path-the-criterion-names-not-an-adjacent-one.md).
That rule is about an AC that *was* implemented but is verified by a test one layer away from
the path it names. This rule is about an AC with no implementation at all — nothing to
mis-test, because nothing was built. Keep the two apart: a coverage gap belongs in the testing
section; a criterion nobody attempted belongs here.

This is also not the out-of-repo-mechanism case — a fix that is correct in this repo but
depends on an unwritten change to a machine-local file (an agent skill, a hook) that no diff
here can contain. That is a sibling rule, tracked under KYO-393, not yet written.

**Rule:** if you did not meet an acceptance criterion, name it in the PR body: which AC, why
it was not met, and where the remaining work now lives — **a ticket ID, not prose.** State "No
deferrals" affirmatively when that's true, so its absence in a PR that has one is itself a
signal, not a blank you can leave for a reviewer to notice on their own. Splitting scope is
normal and often correct — `/backlog-fast` cannot build the WASM bundle or drive a browser, so
Playwright-only ACs are routinely and legitimately split into a follow-up. Splitting it
*silently* is the defect: the gap is then discoverable only by a reviewer who re-reads the
ticket against the diff, which is precisely the audit that does not happen by default. And the
disclosure has to happen at PR-body time, not after a reviewer asks — the reviewer's own
deferral carve-out does not accept a promise to file a ticket. It requires the ticket to
already exist and be cited before the review begins, so waiting to be asked is already too
late to qualify.

```
WRONG — the AC is dropped without a word:

## Summary
- Fixed the popup-opener realm cast so `instanceof` checks work across the
  OAuth popup boundary.

(AC #5 — "a refused window.close() no longer leaves the user with an
unexplained window" — goes completely unmentioned. A reviewer has to notice,
independently, that the ticket asked for something this diff never touches.)

RIGHT — same gap, named:

## Summary
- Fixed the popup-opener realm cast so `instanceof` checks work across the
  OAuth popup boundary.

## Deferred
- AC #3 (live provider handshake verification) not exercised — no
  Snowflake/Databricks credentials available in this environment. Tracked as
  KYO-XXX; to be verified post-merge by the user against a real account.
```

Three review-log entries make the shape of this concrete:

- **KYO-436**, review log `2026-08-22`, `14:10` (initial review, 🟡 MAJOR, category
  "Other: Unmet ticket acceptance criterion"):
  > KYO-436's own AC #5 ("A refused `window.close()` no longer leaves the user with an
  > unexplained window") is explicitly called out in the ticket body and left completely
  > untouched by this diff. No fallback UI, no deferral ticket filed/cited.

  The reviewer would not sign: *"Not signed: MAJOR #1 is an unresolved, unticketed gap against
  the very ticket this PR is closing."* This cost a full review cycle before the fix landed in
  cycle 2.

- **The same PR, same ticket**, `15:05` (re-review, cycle 2) — the contrasting right case,
  from the reviewer's Notes:
  > Implementer separately disclosed, honestly and up front, that no live provider
  > (Google/Snowflake/Databricks) handshake has been manually verified per KYO-436 AC #3, for
  > lack of credentials — treated as a disclosed, not disqualifying, gap; to be stated in the
  > PR body and verified post-merge by the user, not silently dropped.

  Two unmet ACs, one PR. The silent one (#5) was 🟡-blocking and cost a cycle. The disclosed
  one (#3) was accepted without one. The difference was disclosure, not severity.

- **KYO-444**, review log `2026-08-22`, `08:10` — the carve-out itself will not accept a
  promise in place of a ticket, for deferrals of any kind, not only unmet ACs:
  > Not signing — one 🟡 MAJOR (item 1) needs a test before merge; no ticket exists to defer it
  > under the carve-out.

  (That MAJOR finding is a test-coverage gap, not an unmet AC — see
  [cover-the-path-the-criterion-names-not-an-adjacent-one.md](../testing/cover-the-path-the-criterion-names-not-an-adjacent-one.md).
  It's cited here only for what it shows about the carve-out's own bar: an unfiled ticket
  blocks signing regardless of what kind of gap it's covering for.)

Related: KYO-375, review log `2026-08-20`, `17:18`, is the out-of-repo-mechanism case this
rule deliberately excludes — a diff that was itself clean while the ticket it closed depended
on an unwritten change to a machine-local skill file. The reviewer's own framing of why it
still surfaced the gap is the same instinct this rule is written from:
> Flagging loudly per instruction rather than letting a ticket-completion gap hide behind a
> clean diff.

That case is tracked as its own sibling rule under KYO-393 and is not yet written.
