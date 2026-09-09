# A review finding is a claim to verify — not a work order to execute

The code-review-architect is a sub-agent, and its report is the output of a sub-agent: a set
of claims about the tree, produced by an agent that read the tree the same way you can. It
arrives with severity markers, a signature that gates the commit, and the standing rule that
🔴/🟡 must be fixed before signing — all of which make it *read* like an instruction. It is
not one. Every finding is a claim, and every suggested fix is a proposal about how to make
the claim go away.

Both halves can be wrong, and they fail differently. A finding can be false — resolved
against the wrong revision, a stale clone, a file the reviewer never opened — and complying
with it replaces something correct with something incorrect while producing a green cycle
that looks like progress. A finding can be true while the remedy it proposes overshoots:
asserting a cause nobody isolated, adjudicating a disagreement that belongs to another
ticket, restating a claim more broadly than the evidence carries. Adopting that wording
launders the reviewer's inference into your file, where the next reader meets it as
established fact.

The pull toward silent compliance is strong precisely because the reviewer holds the
signature. Rebutting costs a paragraph of evidence and a reviewer who has to re-check their
own work; complying costs one edit and closes the cycle — so complying wins by default. The
loop has no other check: the reviewer will not re-derive a claim nobody contested.

**Rule:** Treat a review finding as evidence, not authority. Before changing a line in
response to one, resolve the finding's own claim yourself, in the tree the branch was cut
from and at the revision under review. If it holds, fix it. If it does not, leave the line
alone and answer in the re-review request with the command and the revision that settle it —
disputing a finding means rebutting it on the record, never editing around it or committing
unsigned — then file a ticket against whatever made the reviewer wrong. When the finding
holds but its proposed remedy asserts more than the record supports, ship the narrower,
evidence-backed version and say plainly that you did not take the wording offered, and why.
When a finding exposes a disagreement that predates your diff, the close is a reconciliation
ticket plus an explicit "this change takes no side" — not picking the side the reviewer's
options implied.

```markdown
<!-- WRONG — the finding says these line numbers are wrong, so they are "fixed" to
     the numbers the finding reported. Nobody re-resolved them; the file now cites
     lines that exist in no revision, and the cycle closes green. -->
Both helpers are on `main` today at `datasources.rs:1897` and `:1911`
```

```sh
# RIGHT — resolve the claim at the revision under review before touching anything.
git show a4ce98e7:crates/kyomi-ui/src/pages/settings/datasources.rs \
  | grep -n 'fn reset_bq_projects_signals\|fn try_reset_bq_projects_signals'
# -> 1971, 1985: the citations stand. The finding read a clone 37 commits behind the
#    branch base. Rebutted in the re-review with this command; line left unchanged;
#    KYO-667 filed against the stale-tree resolution.
```

Four incidents on four tickets, in the `2026-09-05` → `2026-09-09` window. One is the false
finding; the other three are the remedy-overshoot half, and all three were resolved the right
way — which is why they are worth pinning as a practice before the habit erodes:

- **KYO-658**, review log `2026-09-05`, heading *"KYO-658 pre-work: new standard \"a review
  finding names a sample, not the population\""* — the only 🔴 in that entry carries a
  correction appended by the run that requested the review: *"**The 🔴 in this entry is FALSE —
  do not mine a standard from it.**"* The file's `datasources.rs` line citations were correct
  against `origin/main` @ `a4ce98e7`, the revision the worktree was cut from; *"The reviewer
  resolved the file against the canonical clone `/home/jason/repos/kyomi`, whose local `main`
  was `49c1ad5a`, **37 commits behind**, and reported that tree's numbers (`1897`/`1911`)."*
  Tracked as KYO-667. Complying would have written numbers matching no revision into a
  standards file whose subject is citation rigor. Note what did *not* change: *"The outcome
  (drop the file) stands; only the 🔴's reasoning is void"* — the two 🟡s in the same entry
  were correct, and one of them is why the file was dropped. Verifying a finding is not a
  licence to discard the report.
- **KYO-688**, review log `2026-09-09`, heading *"KYO-688 cron sub-agent survival re-test
  (re-review, cycle 2)"* — the implementer accepted the cycle-1 finding and declined the
  wording the reviewer had suggested for the fix, dropping a sentence saying the script's
  indeterminate runs *"predate a known journal gap unrelated to KYO-546."* The reviewer
  agreed on re-review:
  *"this is the correct call, not a shortcut. No evidence in this ticket establishes that
  causal claim… Asserting a specific cause without supporting evidence would reintroduce the
  same inference-presented-as-fact problem the cycle-1 finding was about, just relocated to a
  new sentence."* The reviewer counted it as *"the second reviewed instance in this same log
  of an agent choosing to state a narrower, evidence-backed claim over a broader one a
  reviewer suggested."*
- **KYO-687**, review log `2026-09-09`, heading *"KYO-687: restate the Agent-tool
  `run_in_background` rule as an invariant (kyomi-private/skills)"*, the cycle-2 re-review —
  cycle 1 flagged that the diff endorsed one side of a pre-existing contradiction between two
  skill docs, and offered two ways out. The implementer took neither: it removed the
  endorsement from both sites, filed KYO-691 to own the contradiction, and left a note that
  takes no side. *"Resolution is better than either option I offered: it names the pre-existing
  disagreement, cites the owning ticket, and declines to adjudicate someone else's scope."*
  The reviewer's Notes generalise it: filing the reconciliation ticket and stating "this change
  takes no side" is *"a cleaner close than either endorsing or silently reverting — it keeps
  the PR scope honest and leaves the conflict discoverable."*
- **KYO-703**, review log `2026-09-09`, heading *"KYO-703: remove the PR-listing ceiling from
  check-ticket-in-flight.sh"*, the cycle-2 re-review — cycle 1 found a comment's *"that gap is
  not hypothetical"* evidence over-reaching while its conclusion was independently sound. The
  rewrite conceded the false empirical claim and rested the argument on a structural property
  instead, rather than patching the sentence to survive the finding: *"the restatement is TRUE,
  not merely weaker… Conceding against interest and then winning on structure is a better
  argument, not a hedge."*

Sibling of
[../comments-documentation/re-derive-enumeration-comment-from-source.md](../comments-documentation/re-derive-enumeration-comment-from-source.md):
that rule assumes the finding is true and governs the *blast radius* of the repair — one true
finding about one claim in an enumeration says nothing about its neighbours, so re-derive the
whole comment from source. This one is one step earlier and asks whether the finding, and the
remedy it proposes, should be acted on at all.

Distinct from
[../version-control-working-tree/a-deferral-ticket-is-not-always-enough.md](../version-control-working-tree/a-deferral-ticket-is-not-always-enough.md):
there the finding is accepted and the question is whether a ticket may stand in for fixing it
now. Here the question is prior to that — accept, rebut, or narrow. Distinct from
[../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md](../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md),
which is about an unreproduced claim you author; the KYO-688 bullet is the same claim arriving
in a reviewer's suggested wording, where the temptation is deference rather than convenience.
The verification habit itself is
[../comments-documentation/verify-a-precedent-claim-against-its-source.md](../comments-documentation/verify-a-precedent-claim-against-its-source.md)
turned around and pointed at the review report.

The other rule in this section,
[no-background-subagents-under-headless-run.md](no-background-subagents-under-headless-run.md),
governs how a sub-agent is dispatched; this one governs what its report is worth when it comes
back.

In-flight overlap, disclosed rather than linked: `resolve-a-file-claim-in-the-tree-under-review.md`
(in flight on PR #502, `docs/standards/version-control-working-tree/`) is the *reviewer's* side
of the KYO-658 incident above — resolve a claim in the tree under review before reporting it.
This file is the implementer's side of the same incident: what to do with the report once it is
written. If that file lands, the two should cross-reference on that axis. The same review's other
findings are cited elsewhere for different axes —
[../comments-documentation/re-run-the-duplicate-sweep-at-rebase-not-only-at-authoring.md](../comments-documentation/re-run-the-duplicate-sweep-at-rebase-not-only-at-authoring.md)
cites its duplicate-mining 🟡 — which is the boundary
[../comments-documentation/count-only-the-incidents-that-instantiate-the-rule.md](../comments-documentation/count-only-the-incidents-that-instantiate-the-rule.md)
asks for: the four incidents counted above are the ones where a review finding, or the remedy it
proposed, was the thing acted on.
