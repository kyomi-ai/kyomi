# Read back the whole block you edited — a hunk cannot show you the block

A comment edit is small and self-evidently right: a paragraph inserted, two bullets merged,
a sentence reflowed, a `///` block added above a function. Every finding below was authored
that way, and every one of them is correct *as a hunk*. The defect in each case is a
property of the **assembled block** — where the new paragraph sits relative to the example
it was meant to introduce, whether a sentence eight lines down now says the same thing,
whether a reflow left an orphan line, which item a `///` block ended up attached to — and
none of those properties is visible in the few lines of context a diff prints around the
change. The author reads a hunk; every future reader reads the block.

Four shapes have appeared, in four different file types (a Rust doc comment, a shell script
header, a YAML job comment, a markdown rule file):

- **Placement.** The inserted paragraph landed between a Rule paragraph and the worked
  example that illustrated it, severing an adjacency the block depended on.
- **Attachment.** A `///` block written with no blank line after it silently became the doc
  comment of the *next* item, leaving the function it was written for with none.
- **Wrapping.** A reflow or an insert left an orphan or ragged line mid-sentence in a block
  otherwise wrapped to a consistent column.
- **Redundancy.** Merging two bullets, or inserting a sentence, left one claim stated twice
  a few lines apart. The second copy reads as a new point, and it is the one a later
  "simplify" pass is as likely to keep as the original.

None of these blocked signing — all six were 🟢 — which is precisely why they ship. The
`ci.yml` orphan below was flagged, judged cosmetic, merged with the defect in `8cc48440`,
and sat on `main` for twelve hours until KYO-632's rebase (`f7d7be32`) happened to re-wrap
that sentence for its own reasons. Nobody fixed it; it was absorbed.

**Rule:** After editing prose — a doc comment, a script header, a YAML comment, a markdown
rule file — open the file and read the whole enclosing block, first line to last, rather
than re-reading the diff. Check the four things the hunk cannot show you: that the new text
sits where the block's own flow needs it; that a doc comment is still attached to the item
it names (a blank line ends a `///` block, and a missing one re-parents the entire comment);
that the block is re-wrapped to the width it already used; and that nothing in it is now
said twice. Match the block's existing conventions — emphasis, bullet shape, ordering —
rather than the conventions of wherever the text was pasted from. A block too long to re-read
is the case this rule exists for, not an exemption from it.

```yaml
# WRONG — `git show 8cc48440:.github/workflows/ci.yml`, as merged. An edit
# re-wrapped one line of this paragraph and left its neighbours untouched, so a
# sentence now breaks across an orphan. The hunk was small and read fine.
  # that undercounted the then-five suites by 86 assertions. Note per KYO-609
  # this tally is still
  # hand-maintained and will re-rot on the next suite change; that is a known,

# RIGHT — `git show f7d7be32:.github/workflows/ci.yml`. Same words, the whole
# paragraph re-wrapped as one unit.
  # then-five suites by 86 assertions. Note per KYO-609 this tally is still
  # hand-maintained and will re-rot on the next suite change; that is a known,
```

Six 🟢 findings across five reviews and five tickets, in the `2026-09-02`, `2026-09-03` and
`2026-09-09` review logs:

- **KYO-464** — the `assert-the-count-in-code addition to name-the-invariant-not-a-count.md`
  review. Two findings on one inserted paragraph. Placement: it "sits between the Rule
  paragraph and its own worked WRONG/RIGHT example, which illustrates the leftmost-match case
  from the Rule paragraph, not the fixed-size-array case just introduced … This breaks the
  direct rule→example link for the reader." Convention: "the file's established convention
  italicizes key qualifying phrases … but the new paragraph uses no italics at all, including
  on its own load-bearing qualifier 'genuinely is the contract'." Both were repaired before
  the commit was written — `b2354bd9`'s own message records the decision ("Placed after the
  test-attribution generalisation rather than between the Rule and its own worked example") —
  so what landed in
  [name-the-invariant-not-a-count.md](name-the-invariant-not-a-count.md) is the fixed block.
- **KYO-619** — the `BigQuery missing-key parse fix` review. `paginate`'s doc comment "has no
  blank line before the next `///` block and lands on `fetch_bigquery_list_page` instead;
  `paginate` itself — the function whose loop-termination safety this documents — has no doc
  comment at all." Fixed before merge: on `main`, both functions in
  `crates/kyomi-auth/src/catalog/indexers/bigquery_rest.rs` carry their own.
- **KYO-629** — the `preflight-clippy CI job + ci.yml conflict rewrite` re-review after nit
  fixes. "The reflow left an orphan short line, `# this tally is still`, wrapping mid-sentence
  onto its own line before `# hand-maintained and will re-rot…`." This is the WRONG block
  above, and the only one of the six that reached `main` with the defect intact.
- **KYO-687** — the `restate the Agent-tool run_in_background rule as an invariant` re-review
  whose sole finding is the merged-bullet one (that heading appears three times in the
  `2026-09-09` log; this is the entry with a single 🟢 Copy-Paste finding). Merging two
  bullets in `~/repos/kyomi-private`'s `skills/backlog-fast/SKILL.md` left "costs nothing when
  the notification arrives and saves the run when it does not" near-verbatim in both the
  bullet and a paragraph eight lines below — "harmless, but a future 'simplify' pass could
  delete the wrong copy."
- **KYO-703** — the `remove the PR-listing ceiling from check-ticket-in-flight.sh` re-review
  that flagged `reconcile-merged-tickets.sh` (again one of three entries under that heading in
  the `2026-09-09` log). Both wrapping and redundancy in one edit: the inserted paragraph "was
  not re-wrapped, leaving two ragged short lines mid-paragraph … in a file otherwise wrapped
  consistently at ~78 columns", and the same edit left two consecutive sentences making the
  same point. The finding is against `scripts/reconcile-merged-tickets.sh` as it stood on the
  then-unmerged PR #501 branch, where it was subsequently repaired.

Sibling of
[re-derive-enumeration-comment-from-source.md](re-derive-enumeration-comment-from-source.md):
that rule is about a comment's *claims* — re-read every function the sentence names, because
a finding against one claim does not vouch for the rest. This rule is about the block's form
and internal coherence, where there is no external source to re-derive from: placement,
attachment, wrapping, and a claim stated twice are all decided by the finished text alone.

Sibling of
[an-ordinal-in-a-comment-collides-with-every-concurrent-addition.md](an-ordinal-in-a-comment-collides-with-every-concurrent-addition.md):
there the sentence is accurate when written and invalidated later by someone else's merge, so
no care at authoring time can prevent it. Here the defect exists the moment the edit is saved,
and one read of the block finds it — which is why the remedy is a read-back rather than a
different way of phrasing the sentence.

Distinct from
[re-run-the-duplicate-sweep-at-rebase-not-only-at-authoring.md](re-run-the-duplicate-sweep-at-rebase-not-only-at-authoring.md):
that rule's duplication is a second *file* in the corpus, invisible to any diff, and its
remedy is `git rm`. This rule's duplication is a second *sentence* inside one block, visible
to anyone who reads the block, and its remedy is deleting one of the two copies.

See also
[../version-control-working-tree/prove-a-conflict-resolution-conserved-every-line.md](../version-control-working-tree/prove-a-conflict-resolution-conserved-every-line.md):
two of the findings above came out of a conflict rewrite or a rebase. That rule asks whether
the resolution *kept* every line; this one asks whether the lines it kept still read as a
block once reassembled.
