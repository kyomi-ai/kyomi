# A deferral ticket must be linked to the ticket it came out of — and must not be work you could do in the open diff

Filing a follow-up ticket for something you found mid-task is the right move, and
[state-the-acceptance-criterion-you-did-not-meet.md](state-the-acceptance-criterion-you-did-not-meet.md)
covers the disclosure half: name the gap in the PR body, with a ticket ID rather than prose,
before a reviewer has to ask. That rule stops at "the ticket exists and is cited." Two
further things decide whether the deferral actually holds up, and both have cost review
cycles.

The first is **discoverability**. A ticket that exists but has no relation to the ticket it
came out of is findable only by whoever remembers filing it. Nobody reading the parent ticket
six weeks later — reviewing its fix, triaging a regression against it, or deciding whether the
work is really done — sees the spinoff. The `relates_to` edge is not paperwork; it is the
entire mechanism by which the deferral remains visible after the PR that created it merges.
The same holds for prose: a doc or comment that gestures at "a follow-up ticket" without the
ID is a dead end.

The second is **whether it should have been deferred at all**. The carve-out exists for work
that genuinely does not belong in this diff — a different file, a different subsystem, new UI,
something needing a browser this flow cannot drive. It is not a way to hand a reviewer a
finding you could have fixed in four lines in a file the diff already touches. A well-written
ticket does not make an unnecessary deferral acceptable; the reviewer's objection is to the
deferring, not to the ticket's quality. And a defect the diff *introduces* is never a
candidate: that is a regression, and it gets fixed before signing.

**Rule:** When you defer a finding to a ticket, add the Trakkt relation (`relates_to`, or
`blocks` where it genuinely blocks) in both directions, and cite the ID — never bare prose —
everywhere you point at the deferred work: PR body, code comment, design doc. Before filing,
ask whether the fix reuses machinery that already exists in a file this diff already opens; if
it does, fix it here and file nothing. If the finding is a regression this diff introduced,
fix it here regardless of size.

```
WRONG — the ticket exists, so the box feels ticked:

## Deferred
- The Test & Discover path has the same blind spot; filed a follow-up.

(No ID, so nothing links back. And the fix was an `else if` on an existing
signal already rendered in three places, in a file this PR already changes.)

RIGHT — either fold it in, or defer it with an edge:

## Deferred
- Non-BigQuery providers need new per-resource error UI — genuinely out of
  scope. Tracked as KYO-483 (relates_to KYO-466). The BigQuery `projects`
  half is fixed in this PR.
```

Three review-log entries make the bar concrete:

- **KYO-434** (review log `2026-08-22`, `12:45`, initial) — approved with two 🟢 nits, both of
  this shape: the `DESIGN.md` follow-up cross-reference was "by prose only, not by ID", and
  "KYO-441 has no formal Trakkt relation to KYO-434." Cycle 2 (`13:05`) fixed both — the row
  now names the ticket, and the reviewer confirmed live that "each ticket's `relations` array
  now lists the other."
- **KYO-466** (review log `2026-08-23`, `17:22`, 🟡 MAJOR, blocked signing) — the deferral was
  a genuinely good ticket and still did not clear the bar, on both counts at once. The fix
  "reuses an *already-existing, already-rendered-in-3-places* signal ... with ~4 new lines in
  the same file this diff already touches — squarely inside the carve-out's 'trivially fixable
  in the same diff' exception," and "KYO-483 is also not linked to KYO-466
  (`has_relations: false`), failing the ticket-and-verify carve-out's discoverability
  criterion on its own terms." The reviewer's Notes draw the line explicitly: *"the ticket
  itself is good work, my disagreement is with deferring rather than folding in, not with the
  ticket's quality."* Cycle 2 folded in the BigQuery half and narrowed KYO-483 to the part
  that really was out of scope, with the relation added.
- **KYO-440, cycle 4** (review log `2026-08-24`, `06:15`) — the regression case. Implementing
  the recommended fix surfaced a duplicate-toast defect the diff itself introduced; it was
  "directed to be fixed in-diff rather than deferred since it's a regression this diff
  introduces." The residual, genuinely-out-of-scope remainder became KYO-524, verified by the
  reviewer to be accurately scoped and labelled `agent-ready` — not `deferred`.

Note the label trap while you are here: a spinoff is `agent-ready`, never `deferred`.
`deferred` is Jason's scheduling decision, and applying it to a discovery removes the ticket
from triage and from every `agent-ready` sweep at once — worse than not filing it, because it
looks handled.
