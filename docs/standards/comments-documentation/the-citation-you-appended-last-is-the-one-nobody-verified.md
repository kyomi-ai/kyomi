# The citation you appended last is the one nobody verified

A mined rule's evidence section is not written in one pass. The flagship incident goes in
first and gets the hardest scrutiny — the log entry opened, the code block diffed against the
commit, the heading grepped. The second and third go in next, still inside the verification
mindset. Then, usually late and usually to make the recurrence claim look less thin, one more
bullet is appended from memory of a review the author half-remembers. By then the checking
pass is over. That bullet ships unopened.

The header count is the mirror image of the same mechanism. *"Real precedent — five tickets,
four log days"* is written once, against the list as it stands at that moment, and then the
list moves: a bullet is appended at the end of authoring, or a reviewer's finding removes one.
Nobody recounts, because the count did not feel like part of the edit. The tell is diagnostic
— a count that is exactly right once you delete the last bullet is a count that was written
before that bullet existed.

This deserves its own rule rather than a louder restatement of "verify your citations",
because in every case below the author *was* verifying. The flagship quotes were confirmed
verbatim; reviewers said so explicitly. The discipline was applied, just not uniformly, and
the part it ran out before is structurally predictable. What is missing is not diligence, it
is a position to point at.

**Rule:** Treat the last item in an evidence list and any count that summarises the list as a
separate, final verification pass, run after the list stops changing. Re-open the source for
the most recently appended bullet specifically — the one you are surest of, because you just
wrote it — and test it against your own **Rule:** paragraph, not just against whether the
quote is accurate. Re-derive every number in the section header (tickets, findings, log days,
cycles) from the bullets that actually survived, rather than from the ones you had when you
typed the header. Do both again whenever a review cycle adds or drops a bullet: a deletion
falsifies the header exactly as an addition does.

```markdown
<!-- WRONG — illustrative, not a quotation. This is the file's own cycle-1 draft
     (KYO-697 branch, `an-acceptance-criterion-can-be-false-deviate-on-the-record.md`),
     reconstructed from the review finding rather than quoted — the flawed form
     below was never committed, so there is no `<sha>^` to show. It has since
     landed on `main`, in its corrected (cycle-2) form only; the header was
     written when the list had four bullets, the fifth was appended afterwards,
     and was neither re-counted into the header nor tested against the rule it
     was offered as evidence for. -->
Real precedent — five tickets, four log days:

- **KYO-692 / KYO-713** — …          <!-- opened, quoted verbatim, checked -->
- **KYO-679** — …                    <!-- opened, quoted verbatim, checked -->
- **KYO-704** — …                    <!-- opened, quoted verbatim, checked -->
- **KYO-728** — …                    <!-- opened, quoted verbatim, checked -->
- **KYO-664** — the ticket asked for six files; two were duplicates, so the
  reviewer endorsed shipping four.   <!-- appended from memory; both numbers are
                                          reversed, and the ticket's own AC
                                          instructed the disposition this bullet
                                          describes as a deviation, so it is not
                                          an instance of the rule at all -->

<!-- RIGHT — the unverifiable bullet is removed rather than narrowed, and the
     header is re-derived from the list that survived instead of decremented. -->
Real precedent — five tickets across four log days (`2026-09-09`, `2026-09-11`,
`2026-09-12`, `2026-09-13`):
```

Re-derive the header mechanically from the staged file rather than from recall, as the last
thing you do before staging:

```sh
f=docs/standards/<section>/<slug>.md
grep -c '^- \*\*'                      "$f"   # bullets that survived review
grep -o 'KYO-[0-9]\+'                  "$f" | sort -u | wc -l   # distinct tickets
grep -o '20[0-9][0-9]-[0-9][0-9]-[0-9][0-9]' "$f" | sort -u     # distinct log days
```

Real precedent — three instances across two standards-mining commits, and the third happened
*after* the pattern had been named:

- **The KYO-705 branch's standards-mining commit, `2026-09-10` log, the
  `two new rules from KYO-683 Phase 1 review` entry and the `— re-review` entry under the same
  title** — 🟡, blocked signing.
  `a-dml-migration-needs-its-own-regression-test.md` listed three precedents. The first two
  demonstrated exactly the `Migrator`-restricted-to-version pattern the rule prescribes. The
  third, `agent_learnings_superseded_by_on_delete.rs`, used a materially different technique
  (the full chain against a scratch Postgres database) *and* was a DDL constraint fix rather
  than the UPDATE/DELETE disposition logic the rule's own paragraph defines two sentences
  earlier. The reviewer's summary — *"the citation is 2-for-3, not fabricated wholesale"* — is
  the distribution this rule is about. The re-review entry records the disposition as deletion
  rather than a caveat; the landed file in `../testing/` carries two precedents today.
- **The KYO-697 branch's standards-mining commit, `2026-09-14` log, the
  `an-acceptance-criterion-can-be-false-deviate-on-the-record` entry** — 🟡 on the final
  precedent item and 🟢 on the header count, in the same review. The reviewer verified the
  bullet against the ticket and the log and found its two numbers reversed, and then found the
  larger defect underneath: the ticket's own acceptance criterion *instructed* the disposition
  the bullet presented as a deviation, so it was literal AC compliance, not an instance of the
  rule. The header said four log days while five were cited, and the finding names the
  mechanism: it *"is exactly correct once the KYO-664 bullet from #1 is dropped … which
  suggests the bullet was appended after the count was written."* The entry's Notes state the
  shape outright — *"the flagship citations get verified and the last one appended does not"* —
  and prescribe the remedy this rule carries: *"re-verify the bullet added last, and re-derive
  any count in the header after the list stops changing."*
- **The same commit's cycle-2 entry, `2026-09-14` log** — the prediction held on the next pass,
  which is the strongest evidence available that the position and not the author is the
  variable. The bad bullet was deleted, the header recounted correctly, and the single residual
  finding landed *again* on the now-last-listed bullet: an inference about a PR body that its
  cited source does not carry. The same cycle's two author-found corrections were, in the
  reviewer's words, *"counts/quotes of exactly the kind cycle 1 predicted"* — a "four times"
  that was verbatim three times, and a *"turned out to conflict with"* whose own cited source
  says the opposite. The difference cycle 2 made was not fewer defects in the tail; it was that
  the author looked there and disclosed what they found.

The same position one granularity up is on record and is deliberately **not** counted as an
instance here, because the unverified artefact is a file rather than a bullet: KYO-664's
`land six orphaned coding standards rescued from stranded/dead worktrees` review (`2026-09-10`
log) blocked signing on stale `file:line` anchors in one of six batched files — one of the two
in the batch that had no originating ticket and so had never had independent scrutiny — and
closed by generalising it: *"when multiple orphaned files are batched into one landing PR, each
needs the same anti-staleness pass the flagship file argues for, not just the ones a reviewer
happens to spot-check."* The flagship in question is
[../version-control-working-tree/rescued-work-is-stale-by-construction.md](../version-control-working-tree/rescued-work-is-stale-by-construction.md).

Sibling of
[verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md):
that rule is the obligation — open the thing you cited. This one is where the obligation runs
out. In all three cases above it was honoured for most of the list; reviewers confirmed the
early citations verbatim. Uniform compliance remains the goal, and this rule is the admission
that attention is not uniform, so the thin end needs naming.

Sibling of
[count-only-the-incidents-that-instantiate-the-rule.md](count-only-the-incidents-that-instantiate-the-rule.md):
that rule is the *test* each citation must pass — is this evidence for **this** proposition,
or for a sibling rule's. This one is about *when* you apply that test and to which item. Its
own instruction to "re-derive the whole list rather than decrementing the number" when a
reviewer removes a citation is the same recount, asked for at review time; this rule asks for
it at authoring time too, because the list changes once before any reviewer sees it.

Sibling of
[name-the-invariant-not-a-count.md](name-the-invariant-not-a-count.md): there, the remedy for a
fragile tally is to state the property instead. Here the tally *is* the recurrence claim and is
the reason the file earns a slot in the corpus, so it cannot be replaced by a property — only
re-derived after the list has stopped moving.

Distinct from
[read-back-the-whole-block-you-edited.md](read-back-the-whole-block-you-edited.md): reading the
assembled section back catches a header that no longer matches the bullets beneath it, but it
cannot catch the bullet itself. A late citation reads well — it was written by someone who
believed it. The remedy here is to re-open the cited source for that one item, not to re-read
your own prose.

See also
[cite-a-review-log-entry-by-ticket-and-heading-not-by-timestamp.md](cite-a-review-log-entry-by-ticket-and-heading-not-by-timestamp.md):
the late citation is the one most likely to carry a coordinate the author never followed, and
a re-check is only possible if the entry is addressable at all. And
[a-correct-conclusion-does-not-vouch-for-its-supporting-fact.md](a-correct-conclusion-does-not-vouch-for-its-supporting-fact.md):
when the late bullet turns out not to be an instance, delete it and say why, rather than
quietly narrowing it — in the flagship above, narrowing was not even available, because the
incident did not instantiate the rule at any width.
