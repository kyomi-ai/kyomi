# A correct conclusion does not vouch for the fact offered in its support

A comment that argues for a decision — why the bounded query was rejected, why these check
numbers must not be renumbered, why this window can be treated as clean — is read for its
conclusion. When the conclusion is right, reading stops. The concrete fact offered underneath
it is never checked, and that fact is the part most likely to be wrong: it was sampled once, at
authoring time, from a corpus, a tool run, or a claim about what some other file says, and
nothing re-derives it afterwards.

That produces the most durable kind of documentation defect. A stale comment eventually
contradicts the code and somebody notices. A false premise under a true conclusion contradicts
nothing — every future reader confirms the decision, agrees, and moves on. The reviewer who
caught the flagship case named it exactly:

> The original comment reached a correct conclusion via a false supporting fact — the most
> durable kind of documentation defect, since the conclusion keeps being right and so nobody
> re-checks the premise.

It is not cosmetic. In that same incident the discredited fact had already been relayed to the
user as *the decisive reason* for the design choice, so what shipped was not a stale sentence
but an argument resting on evidence that did not exist.

**Rule:** When a comment exists to justify a decision, rest the justification on a property that
can be re-derived from the code or the tool itself — a predicate the script actually evaluates,
a search qualifier's documented anchoring — rather than on an observation you sampled once. If
you do cite a sampled fact, report the *whole* result, including the part that cuts against you:
the tool's own verdict as well as the column you liked. And when a supporting fact turns out to
be false, do not quietly delete it — state in the comment that it does not hold and why, or the
next author will rediscover the same plausible-looking evidence and put it back.

```sh
# WRONG — a reconstruction. The pre-fix paragraph exists in no commit (it was
# repaired during review, before efb3d62e was written); only the fragment the
# review log quotes, "that gap is not hypothetical", is verbatim here.
#
# A bounded `head:` query would miss PRs whose branches do not follow the
# naming convention, and that gap is not hypothetical: five
# `stranded/jason/kyo-*` refs exist on the remote today.
#
# The five refs are real, and they are *branches*. `mark-branch-stranded.sh`
# refuses to tombstone a branch that has a PR in any state, so by construction
# none of them can be a PR head ref — they are not an instance of the gap at
# all. The conclusion was right anyway, carried entirely by the next sentence,
# which is why nobody re-read the evidence for it.
```

```sh
# RIGHT — quoted from scripts/check-ticket-in-flight.sh at efb3d62e, in the
# header's `WHY NOT A BOUNDED QUERY` section.
# No PR in the current corpus would be missed by that narrowing: of 500 PR
# head refs, the 39 that do not start with `jason/` (`fix/*`, `dependabot/*`,
# …) contain no `kyo-<NN>-` key at all. The objection is that the fetch would
# then be correct only for as long as that convention holds, enforced by
# nothing, while `matches_ticket` deliberately assumes the opposite. Do not
# reach for the `stranded/jason/kyo-*` refs as a counter-example either:
# `mark-branch-stranded.sh` refuses to tombstone a branch that has a PR in
# any state, so those refs are unreachable by any PR listing by construction.
```

Three moves, in that order: state the fact that cuts against you (no PR would actually be missed
today), shift the weight onto a property the code carries (`matches_ticket` matches anywhere in
the ref, GitHub's `head:` is prefix-anchored), and name the discredited argument so it cannot be
re-inserted. The third is the one usually skipped, and it is the one that stops the same wrong
evidence coming back.

The other shape is selective reporting of a real result — quoting the column that supports you
and omitting the tool's own verdict. `scripts/audit-agent-run-deaths.sh`'s header shows the
repair:

```sh
# Conclusion: every `killed.system > 0` run on record predates the 2.1.258
# upgrade — among the runs this script COULD assess, the specific failure
# mode it exists to detect did not recur. That is a narrower claim than "the
# audit came back clean": the script itself did not exit clean for either
# window, because it deliberately refuses to count an indeterminate run as a
# clean one rather than guessing which way it would have gone.
```

Four findings across four tickets in the `2026-09-03` and `2026-09-09` review logs. Only one was
🟡; the other three were 🟢 and none blocked signing, which is exactly why this shape ships.
Three of the four are shell-script headers — two of them the same file — so the sample is
narrower than the rule, but the fourth is a markdown rule file, and nothing about the failure is
specific to shell:

- **KYO-703** — `remove the PR-listing ceiling from check-ticket-in-flight.sh`. Three entries in
  the `2026-09-09` log share that heading; the finding is in the one whose table carries a
  `technical-accuracy-in-comment` row, and the repair is described in the one whose Notes open
  *"Worth recording the shape of the 🟢 #2 fix."* The `stranded/jason/kyo-*` refs above. Both
  code blocks in this file come from it.
- **KYO-688** — `cron sub-agent survival re-test (audit comment + repro script + standard)` and
  its cycle-2 entry, same log. The one 🟡. The header claimed a re-run *"reports ZERO runs with
  `killed.system > 0` across 64 cron runs"*; reproduced live, the count was right and the
  script's own bottom line was `RESULT: COULD NOT COMPLETE`, exit 3, because two runs were
  indeterminate. The reviewer's framing: *"A future reader who trusts the comment (the entire
  premise of these standards) walks away believing the audit is clean; running the exact cited
  command shows it is not."* The RIGHT block above is what shipped.
- **KYO-607** — `recycled pre-restart ticket keys in check-ticket-in-flight.sh`, cycle 3 (three
  entries share that heading in the `2026-09-03` log; this is the one with a single 🟢). The
  header justified keeping the conceptual check numbering because the numbers are *"used
  everywhere in this file, in `scripts/README.md`, and in both skill files"* — neither skill file
  numbers them, and `backlog-fast` uses "check 1 / 1b / 2 / 3" for an unrelated sequence of its
  own. *"The don't-renumber argument survives intact … only the word 'used' overreaches."*
- **KYO-682** — `panic reports ship an empty console_errors array`, cycle 2 (the entry with one
  🟢). Not a script: a sibling-disambiguation paragraph in
  [../agent-orchestration/an-instruction-must-name-a-mechanism-that-exists.md](../agent-orchestration/an-instruction-must-name-a-mechanism-that-exists.md).
  *"The new fourth-sibling paragraph's conclusion is true, but the reason given for instance 2 is
  under-tight"* — it argued the instance was out of a sibling rule's reach because it was prose,
  when that rule explicitly covers prose. The real reason is that the instance names no
  identifier at all, which is what the file says today.

Counted against
[count-only-the-incidents-that-instantiate-the-rule.md](count-only-the-incidents-that-instantiate-the-rule.md),
two nearby findings were considered and excluded. KYO-607's earlier cycles flagged the same
header's *counts* ("over all 474 PRs", "climbs monotonically") with the same "safety argument
still holds" framing, but the reviewer routed them to the sibling rule below and the fix was to
delete the tallies — they belong there, not here. A `name-the-check-you-could-not-run.md` finding
alleging added emphasis inside a quoted excerpt was checked at source and does not hold: the
`2026-09-05` entry it cites does italicise the word.

Sibling of [name-the-invariant-not-a-count.md](name-the-invariant-not-a-count.md): that rule is
this one narrowed to a tally, and its remedy — name the invariant instead of counting — is the
second of the three moves above. It does not reach a supporting fact that is not a number: an
example set, a corpus observation, one column of a tool's output, or a claim about what a
different file says. Reach for that rule when the prop is a count, this one when it is anything
else, and note that neither is discharged by making the number right.

Distinct from
[a-resolving-identifier-is-not-a-verified-claim.md](a-resolving-identifier-is-not-a-verified-claim.md)
and [verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md):
both are about a citation that does not survive being opened, and both are discharged by opening
it. Two of the four findings here would pass that check. The `stranded/jason/kyo-*` refs exist,
`audit-agent-run-deaths.sh` really did report zero `killed.system` runs, and the sentence naming
each was accurate — the defect is that the true fact does not establish the conclusion it was
offered for, which no amount of re-reading the source reveals. Those rules ask *is this true?*;
this one asks *does it prove what it is standing under?*

Distinct from
[no-guarantee-stronger-than-code-enforces.md](no-guarantee-stronger-than-code-enforces.md): there
the overclaim is the conclusion itself — a comment asserting an invariant the code does not
enforce. Here the conclusion is correct and the overclaim is underneath it, which is why review
does not catch it: the thing being checked is the part that is right.

See also
[../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md](../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md)
— its second failure ("the doc was quoted correctly and the conclusion drawn from it is still
wrong") is this rule pointed at external tools, where the remedy is to run the tool; the remedy
here is to move the weight off the observation entirely. And
[../error-handling/empty-on-failure-must-not-look-like-a-real-result.md](../error-handling/empty-on-failure-must-not-look-like-a-real-result.md),
which the KYO-688 reviewer cited: a script that fails closed and then has its refusal omitted
from the comment quoting it has had its fail-closed signal thrown away in prose rather than in
code.
