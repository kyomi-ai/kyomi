# Withdrawing a claim means withdrawing it from everything that carried it

A review finding arrives against one sentence, so the fix edits that sentence. That is the
whole of the repair when the defect is a typo or a stale number. It is not enough when the
sentence carried a *claim*, because a claim that survived long enough to be reviewed has
grown scaffolding: the sentence that introduces or justifies it, the heading that summarises
it, the worked example that illustrates it, and — the part that keeps costing cycles — other
tracked files that were written to agree with it, some of which the corrected text still
points at as its own evidence or as the place to go re-check.

None of that scaffolding is in the diff, and every piece of it was true when it was written.
So once the correction lands the repository asserts both the claim and its replacement, with
nothing wrong in any individual file for a reader to notice in isolation. Worse, the
corrected document is usually the thing that sends the reader to the stale one: *"re-run this
script after any upgrade"*, *"see fact 1 in that header"*. The reader follows the pointer the
new text gave them and lands on the framing the new text just withdrew.

Two shapes recur, and the second is the one nobody expects:

- **The scaffolding outlives the claim.** The claim is deleted or narrowed and the sentence
  that justified it stays, now justifying nothing — or actively contradicting its
  replacement. Headings are the same hazard one level up: a section titled with the old
  conclusion sits above a body that no longer reaches it.
- **A citation made *for* a claim becomes a citation *against* its replacement.** An earlier
  cycle cited a sibling script or doc as corroboration. Withdraw the claim and that
  corroboration silently inverts into a contradiction — frequently in a file the same diff is
  already editing, a few lines away from the hunk.

The second shape is invisible to the obvious searches. There is no renamed symbol to grep
for, nothing fails to compile, and the stale file reads as confident and well-sourced,
because it was sourced — from the claim you have just retracted.

**Rule:** When a review makes you withdraw or narrow a claim, treat the claim's whole
evidence chain as part of the same edit, not just the flagged sentence. Re-read, in this
order: the sentence that introduces or justifies the claim; the sentence that follows it;
any back-reference pointing at it ("that", "the above", "see fact 1 above", "for this
reason"); the heading above it; the WRONG/RIGHT example that demonstrates it; every
artefact the *previous version* cited in support of the withdrawn claim, even if the
replacement drops that citation; and every artefact your corrected text now names as its
source or its re-check entrypoint. Correct each in the same commit, or state plainly in
the corrected text that the other artefact disagrees and why. If an artefact is out of this PR's reach, file the
ticket and name it — do not leave two tracked files quietly asserting opposite mechanisms.

```sh
# WRONG — `git show 518b65cb^:scripts/repro-headless-subagent-survival.sh`,
# verbatim. The standard this script backs was rewritten in the same diff to
# withdraw exactly this generalisation ("the harness keys availability on
# configuration gates, not on session mode"), and that standard names this
# script as the post-upgrade re-check entrypoint — so the correction's own
# pointer lands the next reader on the withdrawn framing.
#      the schema in an INTERACTIVE session on the identical harness version
#      (`description, isolation, model, prompt, subagent_type`) — so the
#      parameter's availability is mode-dependent, not universally present
#      or universally absent.

# RIGHT — `git show 518b65cb:scripts/repro-headless-subagent-survival.sh`,
# merged in PR #505. The two concrete schema observations above the clause are
# byte-identical between the two revisions (diffed); only the generalising
# clause moved, and it now states the mechanism the standard states, names
# the correlation as a correlation, and links the standard.
#      the schema in an INTERACTIVE session on the identical harness version
#      (`description, isolation, model, prompt, subagent_type`) — so the
#      parameter is not universally present or universally absent. The
#      harness keys availability on configuration gates, not on session
#      mode: ... Mode happened to correlate with those two gates on this
#      box. Read the schema you were handed rather than inferring from
#      session type. See docs/standards/agent-orchestration/
#      a-sub-agent-in-flight-must-not-outlive-its-turn.md.
```

(The RIGHT block elides the clause that quotes the harness's own field doc for the
requested-mode counters, marked `...`; every other character is verbatim from that revision.
Both blocks are quotes, not reconstructions.)

The historical blocks above are pinned to the [correction commit in merged PR
#505](https://github.com/kyomi-ai/kyomi/commit/518b65cb9aa52984c494fe18c3a2937b542de40a).
They describe that revision, not the current harness.

KYO-692 / KYO-713 took five review cycles in the post-rebase correction of PR #505.
The [PR's durable review-cycle summary](https://github.com/kyomi-ai/kyomi/pull/505#issuecomment-5626784758)
and KYO-692's final implementation comment (2026-09-10, 23:25 UTC) record this sequence:

| Cycle | Finding | How the previous version or correction left it behind |
|---|---|---|
| 1 | The WRONG example equated omission of `run_in_background` with passing `true`. | The original example's "or simply omitted under `claude -p` — same default" survived into the rewrite. |
| 2 | The RIGHT example then equated omission with passing `false`; both scripts still suggested the kill mechanism was fixed upstream. | Cycle 1's correction inverted the conflation instead of separating the measurements, and the cited scripts retained the withdrawn inference. |
| 3 | A justification survived after the observation it justified was withdrawn. | Cycle 2 reframed the observation but left its introducing sentence behind. KYO-742 records the surviving clause as "because they do not agree along session type". |
| 4 | `scripts/repro-headless-subagent-survival.sh` still taught that availability was "mode-dependent". | Cycle 3 withdrew mode-keying as the mechanism, turning a script the standard cited twice as authority into a contradiction. |

Cycle 1 exposed a carried-forward defect; cycles 2–4 show corrections leaving or creating
another inconsistency. Cycle 5 was clean of blocking findings. Verifying each replacement
against its evidence did not check the surrounding prose or the sources cited by its
predecessor. KYO-742 records two positive instances from the cycle-4 implementation:

- **Neighbourhood:** after inserting a clause, the implementer noticed that a following
  "that" now had an ambiguous referent and changed it to "that variation" in the same pass.
  The [landed standard at the correction commit](https://github.com/kyomi-ai/kyomi/blob/518b65cb9aa52984c494fe18c3a2937b542de40a/docs/standards/agent-orchestration/a-sub-agent-in-flight-must-not-outlive-its-turn.md)
  retains that explicit referent.
- **Cited artefact:** re-reading the reproduction script also caught its later environment
  paragraph: "the whole point of KYO-688" was said to be differing behaviour under
  `claude -p`, justified by "see fact 1 above". That back-reference reasserted the withdrawn
  mechanism away from the header hunk. The same [correction commit](https://github.com/kyomi-ai/kyomi/commit/518b65cb9aa52984c494fe18c3a2937b542de40a)
  repairs it to describe the observed schemas and their configuration, preventing another
  review cycle rather than waiting for a reviewer to find it.

Sibling of
[re-derive-enumeration-comment-from-source.md](re-derive-enumeration-comment-from-source.md):
there a finding against one claim in a sentence does not vouch for the sentence's *other*
claims, and the remedy is to re-read every function the sentence names. Here the flagged
claim is genuinely the only false one — the problem is everything written to agree with it,
which is still true-sounding and now wrong, and the remedy is to walk the claim's citation
graph rather than its sentence.

Sibling of
[read-back-the-whole-block-you-edited.md](read-back-the-whole-block-you-edited.md): that rule
says a hunk cannot show you the assembled block, and its remedy — open the file and read the
enclosing block — catches the *scaffolding* half of this one. It stops at the file boundary;
the citation-inversion half lives in other files by definition, and no read-back of the block
you edited reaches it.

Distinct from
[a-correct-conclusion-does-not-vouch-for-its-supporting-fact.md](a-correct-conclusion-does-not-vouch-for-its-supporting-fact.md):
that rule is about the premise under a conclusion nobody re-checks *because the conclusion is
right*. It also says that when a supporting fact turns out to be false you must say so rather
than quietly delete it — that instruction applies to the document you are fixing; this rule
extends the same obligation to every other file that was leaning on the fact.

Distinct from
[sweep-for-the-comments-your-change-made-false.md](sweep-for-the-comments-your-change-made-false.md):
that rule is triggered by a **deletion** and has a mechanical handle, the removed identifier,
which `git grep` finds in prose the compiler cannot see. This rule is triggered by a
**correction**, often in a docs-only diff where no identifier changed at all and no grep term
exists — the stale sites are found by following what cited the claim, not by searching for a
name.

See also
[cite-what-has-landed-not-an-open-pr-or-a-stranded-branch.md](cite-what-has-landed-not-an-open-pr-or-a-stranded-branch.md):
when the correction is in flight rather than merged, the sibling artefact's disagreement is
*expected* for as long as the PR is open, and saying so is part of the disclosure this rule
asks for. And
[../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md](../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md):
that rule is why these claims get withdrawn in the first place — a correct observation with
an over-reaching inference bolted on. This rule is about what the withdrawal owes the rest of
the tree.

Distinct from
[verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md)
and
[a-resolving-identifier-is-not-a-verified-claim.md](a-resolving-identifier-is-not-a-verified-claim.md):
those verify that a claim is supported by what it cites, beyond merely resolving a name.
Here the replacement can pass that check and still leave its introducing or following
sentence, a back-reference, or a previously cited artefact asserting the withdrawn claim.
The extra step is re-reading what supported the *previous version* of the claim.
