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
order: the sentence that introduces or justifies the claim; the heading above it; the
WRONG/RIGHT example that demonstrates it; every artefact the *previous* cycle cited in
support of it; and every artefact your corrected text now names as its source or its
re-check entrypoint. Correct each in the same commit, or state plainly in the corrected text
that the other artefact disagrees and why. If an artefact is out of this PR's reach, file the
ticket and name it — do not leave two tracked files quietly asserting opposite mechanisms.

```sh
# WRONG — `git show origin/main:scripts/repro-headless-subagent-survival.sh`,
# verbatim. The standard this script backs was rewritten in the same diff to
# withdraw exactly this generalisation ("the harness keys availability on
# configuration gates, not on session mode"), and that standard names this
# script as the post-upgrade re-check entrypoint — so the correction's own
# pointer lands the next reader on the withdrawn framing.
#      the schema in an INTERACTIVE session on the identical harness version
#      (`description, isolation, model, prompt, subagent_type`) — so the
#      parameter's availability is mode-dependent, not universally present
#      or universally absent.

# RIGHT — the same block on PR #505's head (in flight, not merged as of
# 2026-09-11). The two concrete schema observations above the clause are
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

Real precedent — two tickets, and the flagship cost three of five review cycles:

- **KYO-692 / KYO-713 (PR #505), `2026-09-11` log, the *cycle 2* entry under "rename and
  correct the headless sub-agent standard"** — 🟡. After the standard withdrew "the mechanism
  appears to have been fixed upstream", *both* tracked scripts still concluded it: the
  reviewer found "appears to have been fixed upstream since" surviving in
  `scripts/repro-headless-subagent-survival.sh` and `scripts/audit-agent-run-deaths.sh`,
  noting "the standard names `repro-headless-subagent-survival.sh` as the post-upgrade
  re-check entrypoint, so a future agent following that pointer lands on the refuted
  conclusion." The repro script's surrounding paragraph *was* edited by that same diff.
- **Same ticket, the *cycle 3* entry** — 🟡, the scaffolding shape. The claim that rows
  stratify by session type was withdrawn; the sentence justifying it ("because they do not
  agree along session type") was left behind, now contradicted by the very table beneath it.
  Two 🟢 in the same chain are the heading version of it: a section still titled "What's
  changed since KYO-546" over a body whose conclusion had become a bound, and a
  `DID NOT REPRODUCE` banner fifty lines above the qualification that bounded it.
- **Same ticket, the *cycle 4* entry** — 🟡, and the citation-inversion shape stated exactly.
  Cycle 3 had cited the repro script's "mode-dependent" line as an *ally* — evidence the rows
  really do stratify by session type. The fix then withdrew mode-keying as the mechanism,
  which converted that ally into a contradiction 25 lines from a hunk the same diff was
  making. The reviewer's note is the rule in one sentence: *"a rewrite that withdraws a claim
  must re-read not only the sentence introducing it but every artefact the previous cycle
  cited in support of it — a citation made for a claim becomes a citation against its
  replacement."* That chain — each cycle's finding created by the previous cycle's fix — broke
  only at cycle 5, when both re-reads finally came back clean. **KYO-742** owns this rule.
- **KYO-687, `2026-09-09` log, the *initial* review entry under "restate the Agent-tool
  `run_in_background` rule as an invariant"** — the cross-repository version, twice in one
  diff. 🟢: three docs were corrected and a "**fourth site**, tracked + public, still teaches
  the false rule" — a public standards file, out of that PR's repo, which had to become its
  own ticket (KYO-692, i.e. the flagship above). 🟡: a newly added sentence endorsed a
  practice the *same file* had flatly banned 53 lines earlier; the diff did not create that
  conflict but newly took a side in it without reconciling.

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

Distinct from `a-clean-build-is-not-a-comment-sweep.md` (in flight on
`jason/kyo-727-qa-sweep`, KYO-727 — not on `main` as of 2026-09-11): that rule is triggered by
a **deletion** and has a mechanical handle, the removed identifier, which `git grep` finds in
prose the compiler cannot see. This rule is triggered by a **correction**, often in a
docs-only diff where no identifier changed at all and no grep term exists — the stale sites
are found by following what cited the claim, not by searching for a name.

See also
[cite-what-has-landed-not-an-open-pr-or-a-stranded-branch.md](cite-what-has-landed-not-an-open-pr-or-a-stranded-branch.md):
when the correction is in flight rather than merged, the sibling artefact's disagreement is
*expected* for as long as the PR is open, and saying so is part of the disclosure this rule
asks for. And
[../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md](../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md):
that rule is why these claims get withdrawn in the first place — a correct observation with
an over-reaching inference bolted on. This rule is about what the withdrawal owes the rest of
the tree.
