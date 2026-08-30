# Name the nearest sibling rule inside the new rule — or don't add the file

Every rule in `docs/standards/` is mined from the same source: the last week or two of
`docs/review-logs/`. Several agents mine that window, and they converge — on the same
incidents, in the same order, because the loudest incidents are loud for everyone. Two
agents starting from KYO-446's spliced `assert!` and KYO-455's 74-test split will write
the same rule, with different words and a different slug, and both will be correct.

Correct is not the bar. A section with two rules for one lesson is worse than either
alone: a reader who finds one does not learn the other exists, the pair drifts as each is
amended separately, and `ls docs/standards/<section>/` — the only discovery mechanism
there is, since [the index enumerates sections, not rules](../../CODING_STANDARDS.md) —
shows two entries that look like two lessons.

**Nothing in the process catches it, and the reason is structural.** KYO-375 made adding a
rule "one new file, no existing line touched" precisely so concurrent PRs cannot collide.
That works, and it has a cost the ticket did not anticipate: neither PR's diff can *see*
the other's file, so a semantic duplicate is invisible to both authors and both reviewers,
and only exists in the merged tree nobody is looking at. The duplication is not a review
miss. It is what review cannot see.

So the check has to happen at authoring time, against the whole corpus rather than the
diff — and it has to leave evidence in the file, because the next author is in exactly the
same blind spot you were.

**Rule:** Before writing a rule file, `ls` every section directory — not only the one you
picked — and read every file whose slug is even loosely related. Then:

- **Total overlap** → do not write the file. Say in the PR which existing file covers it
  and close the ticket as a no-op. That is a legitimate, complete outcome.
- **Partial overlap** → keep only the part that is genuinely new, and state the axis
  separating you from the nearest sibling *in the new file*: "Sibling of X: that rule
  covers Y, this one covers Z." One sentence, naming the file.
- **In-flight overlap** → `git log --all --diff-filter=A --name-only -- 'docs/standards/**'`
  after a `git fetch`. A rule approved on an unmerged branch is not in your tree and will
  not show up in any `ls`.

A missing cross-reference is a finding on its own, even when every word in the file is
true and every citation checks out.

```markdown
<!-- WRONG — closes by distinguishing three siblings and omits the one it actually
     overlaps, which shares both its Rule paragraph and its flagship incident. -->
Related: anchor-a-citation-to-a-symbol-not-a-line-number.md,
re-derive-enumeration-comment-from-source.md, no-guarantee-stronger-than-code-enforces.md

<!-- RIGHT — the closest overlap named first, with the axis that separates them. -->
Sibling of verify-a-precedent-claim-against-its-source.md: that rule covers a precedent
claim in prose that nothing can mechanically check; this one covers a citation whose
existence-grep passes while the attached claim is still false.
```

```sh
ls docs/standards/*/                                   # the whole corpus, not one section
grep -rlni 'conflict\|resolution\|conserve' docs/standards/   # your topic's vocabulary
git fetch origin && git log --all --diff-filter=A --name-only -- 'docs/standards/**'
```

Nine reviews in the 2026-08-23 → 2026-08-30 window treat this as a named, first-class
verification step — which is what makes it an established convention rather than one
reviewer's habit — and the one time it was skipped it was the sole blocking finding in a
six-file batch:

- **KYO-533 cycle 1** (`2026-08-30`, `00:33`) — 🟡 Copy-Paste, refused signing.
  `a-resolving-identifier-is-not-a-verified-claim.md` "substantially duplicates the
  pre-existing sibling `verify-a-precedent-claim-against-its-source.md`… both use the
  *identical* flagship incident (KYO-429, cycles 1 and 2)… but never mentions
  [it], despite it being the closest overlap of all four." Cycle 2 (`00:40`) named the
  standing hazard: *"near-duplicate rules landing without a disambiguating
  cross-reference."* The fix was one additive sentence — no existing line changed.
- **KYO-468** (`2026-08-29`, `12:28`) — the case that proves the check is not sufficient as
  practised. `prove-a-conflict-resolution-conserved-every-line.md` was reviewed for exactly
  this and passed: "genuinely additive over the nearest sibling
  (`verify-the-object-that-ships-not-the-working-tree.md`)… explicitly cross-referenced."
  Its real near-duplicate, `prove-a-conflict-resolution-conserved-content.md`, was sitting
  on another branch at that moment. Both landed eighteen seconds apart — `f2c0fe4e`
  (PR #434) and `2f9b5dce` (PR #437) — citing the *same six incidents* (KYO-446, KYO-407 ×2, KYO-452,
  KYO-455, KYO-491). KYO-564 now tracks merging them; KYO-562 tracks the pair plus this
  rule.
- **Done right, six times:** KYO-407 (`2026-08-23`, `00:49`) confirmed a carried-across
  rule "does not already exist on `origin/main` (no duplicate)"; KYO-469 (`08-23`, `13:05`)
  "not duplicative of its sibling… explicitly cross-referenced"; KYO-492 carryover
  (`08-24`, `04:10`) grepped all of `docs/standards/` for the overlapping vocabulary and
  quoted the file's own line drawing the boundary; KYO-456 (`08-25`, `00:27`) confirmed an
  existing testing rule already cited three of the tickets and approved the new file
  *declining to re-cite them*; KYO-463 (`08-27`, `03:50`) "read every other file in both
  sections… No duplication"; KYO-534 (`08-30`, `03:40`) "both cited sibling files exist and
  are correctly distinguished, not duplicative."

Sibling of [verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md)
and the other standards-authoring rules in this section: those ask whether a document's
claims are *true*. This one asks whether the document needed to exist. KYO-533's finding
was explicitly not a truth problem — *"a documentation-architecture issue, not a factual
error — nothing in any of the six files is wrong, fabricated, or unsupported"* — so a file
can satisfy every one of those rules perfectly and still be a redundant restatement.
Distinct too from [third-copy-of-test-helper-is-extraction-trigger.md](../code-organization/third-copy-of-test-helper-is-extraction-trigger.md),
which is about duplicated *code* and triggers on the third copy with extraction as the
remedy: here the second copy is already the defect, and the remedy is a cross-reference or
no file at all.
