# An acceptance criterion can be false — check its premise, then deviate on the record

Every ticket is written against a snapshot: the code as it stood when someone read it, the
ticket status of whatever it depends on, the contents of a file nobody re-opened. By the time
an agent implements it, some of those facts have moved. An acceptance criterion written on top
of a fact that has since changed does not announce itself — it reads exactly like every other
AC, and the cheapest response is to satisfy it literally.

Satisfying it literally is how a false statement ships. An AC that says *"remove every
occurrence of X from the examples"* is a good instruction only while X really is absent from
the thing the document describes; if it is present in one mode, complying makes the document
say something untrue, and the diff still reports every AC green. An AC that says *"cite ticket
N as open"* is correct only until N closes, at which point complying violates
[../comments-documentation/cite-what-has-landed-not-an-open-pr-or-a-stranded-branch.md](../comments-documentation/cite-what-has-landed-not-an-open-pr-or-a-stranded-branch.md).
A standing convention can go the same way: *"adding a standard modifies no existing line"*
exists to stop two concurrent PRs colliding on a shared line, and reading it as a ban on ever
correcting an existing file freezes whatever is wrong in that file forever.

The opposite failure costs just as much. A diff that quietly does something other than what the
AC said — even something better — is indistinguishable, from the outside, from a diff that
missed the AC. The reviewer cannot tell whether you judged the criterion wrong or never read
it, so the finding gets written either way and the cycle is spent establishing which it was.

Three shapes recur:

- **The AC's factual premise has gone stale.** A dependency ticket closed, the file it
  describes was rewritten, the behaviour it assumes was measured on an older build.
- **Two ACs disagree**, usually across two tickets touching the same files, where each is
  correct about its own ticket's scope and neither is correct as an unconditional rule.
- **The scope boundary the AC draws is asserted, not measured.** "Out of phase-A scope" is a
  claim about what the fix would cost; opening the fix settles it, and sometimes it is one line
  in a file already named in the diff.

**Rule:** Before implementing an acceptance criterion, check the fact it rests on against the
code or artefact it describes — the same bar
[../comments-documentation/verify-a-precedent-claim-against-its-source.md](../comments-documentation/verify-a-precedent-claim-against-its-source.md)
sets for a citation. If the premise holds, implement it. If it does not, implement the
behaviour that is true and record the deviation **in the PR body, naming the AC, the fact that
falsifies it, and how you checked** — before a reviewer asks, and ideally before you write the
code, so the deviation is a decision on record rather than a discovery. Restate the standing
deviations in every re-review request: a prior cycle's acceptance does not carry forward, and a
prior reviewer's *description* of a deviation is itself a claim to re-derive. Deviating is only
available where the code settles what the correct behaviour is; when it does not, that is a
decision to route as one, not a deviation to take unilaterally. And a deviation with no false
premise behind it is not covered by this rule — that is scope drift, and
[declare-the-change-the-ticket-did-not-ask-for.md](declare-the-change-the-ticket-did-not-ask-for.md)
governs it.

The blocks below are illustrative PR-body prose, not quotations from any commit.

```
WRONG — the AC is satisfied literally and the document now lies:

## Summary
- AC 1: removed every `run_in_background` occurrence from the examples. ✅
- AC 6: cites KYO-688 as the open question. ✅

(The parameter does exist in one of the two modes this document governs, so the
examples can no longer state the remedy for that mode and the blanket phrasing
that replaced them is false. KYO-688 shipped four days ago, so the citation
points at a question that has an answer.)

WRONG — the same judgement, made silently:

## Summary
- Reworked the examples and the KYO-688 reference.

(Correct as shipped. Nothing here distinguishes "I judged AC 1 wrong" from "I did
not read AC 1", so the reviewer has to establish which — and the deviation is
re-litigated from scratch on every later cycle.)

RIGHT — both deviations named, with the fact that forced each:

## Deviations from the ticket's ACs
- **AC 1** ("no `run_in_background` in any example") not met as written. Read the
  tool schema directly: the parameter is present under `claude -p`, absent
  interactively. Removing it leaves the RIGHT example unable to name the remedy
  in the mode this standard governs, and a blanket "no such parameter" sentence
  would itself be false. Shipped: one occurrence, annotated `claude -p`-only.
- **AC 6** ("cite KYO-688 as open") not met as written. KYO-688 is answered and
  shipped (PR #503) — verified via `gh pr view`, not from the ticket. Citing it
  as open would breach `cite-what-has-landed-...`, so the file cites its finding.
- No other deviations.
```

Real precedent — five tickets across four log days (`2026-09-09`, `2026-09-11`, `2026-09-12`,
`2026-09-13`). Entries are cited by ticket and heading per
[../comments-documentation/cite-a-review-log-entry-by-ticket-and-heading-not-by-timestamp.md](../comments-documentation/cite-a-review-log-entry-by-ticket-and-heading-not-by-timestamp.md):

- **KYO-692 (PR #505), `2026-09-09` log, the entry under "rewrite
  no-background-subagents-under-headless-run.md"** — the flagship, and the only clean-on-the-
  first-pass review in the chain. Two of the ticket's own ACs were false, both flagged before
  implementation. On AC 1 the reviewer: *"This is the correct call — omitting it entirely would
  leave headless readers (the mode this standard governs) without the actual remedy; a blanket
  "no such parameter" statement would itself be false."* On AC 6: *"KYO-688 is in fact answered
  and shipped (PR #503) — citing it as resolved is more accurate than the ticket's stale
  premise."* The entry's verdict is the rule in one line: *"Both deviations improve accuracy
  over literal AC compliance and neither weakens the "will this mislead a copier" test —
  approved as written."*
- **KYO-692 / KYO-713 (PR #505), the `2026-09-09` "conflict-rebase re-review" entry and the
  `2026-09-11` entries under "rename and correct the headless sub-agent standard"** — the same
  three deviations were adjudicated in seven separate review entries across two days, each
  closing with a paragraph re-justifying them from scratch (*"all three upheld"*, four times in
  the `2026-09-11` log alone). Two things that only emerged by re-deriving rather than
  inheriting: AC 7 (*"do not edit… any sibling rule"*) read as conflicting with **KYO-713's**
  AC 4 (*"every sibling rule that cross-references it still resolves"*), resolved by scoping
  each AC to its own ticket rather than by picking a winner; and a later reviewer found the
  *prior reviewer's own description* of the AC-7 deviation wrong — *"the prior reviewer's
  "link-target-only" characterisation is inaccurate… which rewrites two sentences of prose, not
  just a link target. That edit is nonetheless **mandatory** — leaving "as of this writing it
  still carries the defective form, and KYO-692 owns correcting it" would ship a false statement
  the moment this PR merges — so the deviation is justified; only the description of it was
  wrong."*
- **KYO-679, `2026-09-13` log, the entry under "Rule B disposal-safety lint ratchet"** — the
  convention case rather than the ticket case. The diff edited an existing file under
  `docs/standards/`, against the index's "adding a standard creates exactly one new file and
  modifies no existing line". Upheld, with the premise named: the file *"was already asserting
  stale/false claims… updating it is the correct call, not a violation of the "don't edit
  existing rule files" invariant, which is scoped to prevent concurrent-PR collisions on
  new-rule additions, not to freeze accuracy bugs in place."*
- **KYO-704 Phase A, `2026-09-11` log, the initial entry under "retire BigQuery kyomi_oauth,
  default to service_account"** — 🟡, and the check on this rule rather than an instance of it.
  The deviation here was a *refusal* to touch a neighbouring file, justified as out of scope;
  the reviewer opened the fix and measured it: *"One-line fix… not a signature change, so the
  implementer's "out of phase-A scope" justification for not touching this file doesn't actually
  cover it."* A stated justification is a claim with the same standing as any other.
- **KYO-728 phase A, `2026-09-12` log, the entry under "collapse passkey-vs-email signup
  fork"** — 🟢, the counter-example. The brief said a divider is removed; the diff re-gated it
  instead, *"rather than removing it outright, as the phase-A brief's item 1 literally states"*,
  with nothing in the PR explaining why. No false premise was behind it, literal compliance was
  right, and cycle 2 duly removed it. The finding exists because an unexplained deviation reads
  the same as a missed AC even when it is harmless.

Sibling of
[state-the-acceptance-criterion-you-did-not-meet.md](state-the-acceptance-criterion-you-did-not-meet.md):
that rule covers the AC you did not attempt, and its remedy is a disclosure plus a ticket ID
holding the remaining work. This one covers the AC you deliberately did not follow because it
was wrong, where there is no remaining work to track — the disclosure has to carry the
falsifying fact instead, because a reviewer who cannot re-derive that fact has no way to tell
the two situations apart. Mirror of
[declare-the-change-the-ticket-did-not-ask-for.md](declare-the-change-the-ticket-did-not-ask-for.md):
there the diff does more than the ticket asked and the obligation is to name the extension;
here it does something *other* than what the ticket asked, and the obligation is to name the
reason the ticket was wrong.

Distinct from
[a-deferral-ticket-is-not-always-enough.md](a-deferral-ticket-is-not-always-enough.md): a
deferral concedes the ticket is right and postpones the work. A deviation asserts the ticket is
wrong and does different work now, so a follow-up ticket is not the disposition — there is
nothing left to do, and filing one would re-import the false premise.

See also
[../agent-orchestration/a-review-finding-is-a-claim-to-verify-not-an-instruction.md](../agent-orchestration/a-review-finding-is-a-claim-to-verify-not-an-instruction.md)
and
[../agent-orchestration/a-review-request-describes-the-delta-it-does-not-establish-it.md](../agent-orchestration/a-review-request-describes-the-delta-it-does-not-establish-it.md):
a finding, a review request and an acceptance criterion are the same kind of object — prose
from another party, describing the code, that nothing checks by default. This rule is that
scepticism pointed at the ticket. And
[../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md](../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md):
when the premise is a claim about a tool's behaviour, "the ticket says so" settles nothing in
either direction — reproduce it before you comply with it *or* deviate from it.
