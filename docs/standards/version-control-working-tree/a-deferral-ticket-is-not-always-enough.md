# A deferral ticket is not always enough — fix it in-diff when it is cheap, or when you caused it

[State the acceptance criterion you did not meet](state-the-acceptance-criterion-you-did-not-meet.md)
sets the floor for splitting work out of a PR: disclose it in the PR body, and back it with a
ticket that already exists before the review begins. That is the floor, not the ceiling.
Reviewers in this repo have blocked signing on deferrals that cleared the floor exactly — the
ticket existed, it was `agent-ready`, it accurately described the defect — and the deferral was
still refused.

The standing instruction to turn a mid-task discovery into an `agent-ready` ticket rather than
fixing it inline exists to stop unrequested scope expansion: a discovery in a *neighbouring*
function inflates the diff and buries the change under review. It is not a licence to route
around a finding *in the code the diff already changed*. Two things disqualify a deferral:

- **Cost.** If the fix is a handful of lines in a file the diff already has open, reusing
  machinery that already exists elsewhere in that file, the ticket costs more to write, triage,
  schedule and re-review than the fix costs to make — and it leaves the shipped code wrong in
  the meantime. "I filed a ticket" is not an answer to "this is four lines and you are already
  editing this function."
- **Provenance.** If the defect is a regression *this diff introduces*, it is not a discovery at
  all. Deferring it means knowingly shipping a break and asking a future agent to clean up after
  you, with none of the context you have right now.

A deferral that survives both tests still has to be discoverable. A follow-up ticket with no
relation to the ticket it was spun out of is invisible to anyone reading the original — it
satisfies the letter of the carve-out and none of its purpose. Link it (`relates_to`, or
`blocks` when it genuinely blocks), and narrow its scope to the part you actually deferred, so
the ticket does not still claim work that shipped.

**Rule:** before deferring a review finding to a ticket, apply both tests. If the fix is small
and lives in code this diff already touches, or if the defect is a regression this diff
introduces, fix it in the same PR and re-request review — do not file a ticket. When a deferral
does survive both tests, file it, link it to the originating ticket, narrow it to the remaining
scope, and cite the ID in the PR body. A ticket is a routing decision, not a way to close a
finding.

```
WRONG — the ticket exists, is well specced, and the deferral is still refused:

## Deferred
- The Connection-tab "Test & Discover" effect has the same blind spot on its
  own path. Tracked as KYO-483.

(The fix is an `else if` off an existing read, populating a signal that is
already declared and already rendered in three places, in a file this diff has
open. And KYO-483 carries no relation to this ticket, so nobody reading the
original will ever find it.)

RIGHT — split on cost, not on convenience:

## Summary
- ... plus the sibling Connection-tab path, which needed four lines against the
  existing `bq_projects_error` signal.

## Deferred
- The equivalent gap for non-BigQuery providers is genuinely larger (new
  plumbing per provider). Narrowed KYO-483 to that scope and linked it to this
  ticket.
```

Four review-log entries, three tickets, five days:

- **KYO-466**, review log `2026-08-23`, `17:22` (initial, 🟡 MAJOR, "incomplete fix / second
  consumer left blind"). The implementer had already found the second consumer and filed a
  ticket. The reviewer refused it anyway:
  > The implementer correctly identified this and filed KYO-483, but I judge the deferral
  > doesn't clear the bar: the fix reuses an *already-existing, already-rendered-in-3-places*
  > signal (`bq_projects_error` …) with ~4 new lines in the same file this diff already touches
  > — squarely inside the carve-out's "trivially fixable in the same diff" exception. KYO-483 is
  > also not linked to KYO-466 (`has_relations: false`), failing the ticket-and-verify
  > carve-out's discoverability criterion on its own terms.

  Cycle 2 (`17:45`) is the resolved shape: the BigQuery half was fixed in-diff, and KYO-483 was
  "confirmed narrowed to the remaining other-providers scope with a `relates_to` link to
  KYO-466." Both halves of the rule, in one ticket.

- **KYO-440**, review log `2026-08-24`, `13:49` (cycle 2) — the cost test stated as a
  recommendation rather than a finding:
  > Recommend the implementer address the fix suggested above in the same PR before the next
  > review pass, since it's the same file/function already open and is squarely a "trivially
  > fixable" scope, not a defer-worthy one.

- **KYO-440**, review log `2026-08-24`, `06:15` (cycle 4) — the provenance test, applied to a
  regression the fix itself introduced. Implementing the recommended split meant a row-private
  token could no longer observe a `postMessage`, so a duplicate "connection cancelled." toast
  appeared over an accurate one:
  > (KYO-524, found by the implementer, directed to be fixed in-diff rather than deferred since
  > it's a regression this diff introduces).

  KYO-524 survived as a ticket only for the genuinely different residual gap left over
  afterwards — and the reviewer re-fetched it to confirm its title and description had been
  rewritten to describe that narrower gap rather than the fixed one.

- **KYO-386**, review log `2026-08-20`, `20:29` (cycle 2) — the same call on a one-line shell
  fix, made before any ticket was written:
  > This is a small, mechanical fix — recommend applying it and re-requesting review rather than
  > deferring; not worth a ticket for a one-line change in an unmerged diff.

The contrasting case, so the rule is not read as "never defer": KYO-441 (review log
`2026-08-22`, `12:45`) was a real `z-50` gap in two components the KYO-434 diff did not touch.
The reviewer verified it via `mcp__trakkt__get_issue` — "real, `agent-ready`+`bug` (not
`deferred`), accurately specced with file:line, a concrete failure scenario … and acceptance
criteria" — and recorded that this "confirms this discovery was scoped correctly rather than
fixed inline or silently dropped." Different file, real design work, correctly deferred.

Sibling of [state the acceptance criterion you did not meet](state-the-acceptance-criterion-you-did-not-meet.md):
that rule is about *disclosing* a gap; this one is about which gaps you are allowed to have.
When the deferred item is specifically a missing test for a path the ticket names, see
[cover the path the criterion names, not an adjacent one](../testing/cover-the-path-the-criterion-names-not-an-adjacent-one.md).
