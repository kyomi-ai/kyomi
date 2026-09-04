# Count only the incidents that instantiate the rule

A mined standard closes with an evidence paragraph — "four 🟡 findings across two tickets in
one week" — and that paragraph does two jobs at once. It sources each claim, and it asserts a
*recurrence*: this shape happens often enough to be worth a rule. The second job is the one
that decides whether the document should exist at all, and it is the one nothing checks.

Every individual citation can survive scrutiny and the paragraph still be wrong. Each entry
resolves, each summary of it is accurate against the log, each is a real finding on a real
ticket — and the set is still not four instances of *this* rule. What happened is that the
author collected the findings they remembered from the same review session, which were
adjacent in time and superficially similar in shape, rather than the findings that the Rule
paragraph, as written, actually describes. The remainder are usually instances of a sibling
rule the document itself names two paragraphs later as *distinct from* this one.

The cost is not cosmetic. A rule justified by four incidents reads as settled; the same rule
justified by one reads as a hypothesis, and a hypothesis with one instance should usually wait
for a second rather than land. Inflating the count converts "we should watch this" into "this
is a known recurring defect" without anyone deciding to. It is also self-undermining in this
corpus specifically: a standard about precision whose own evidence is imprecise teaches the
reader to discount it.

**Rule:** Before you write the recurrence count, test each cited incident against your own
**Rule:** paragraph one at a time and ask what the fix was — not whether it *feels* like the
same family. If the incident's remedy belongs to a sibling rule, it is that rule's evidence,
not yours; drop it and re-count. State the number that survives. If one survives, either find
a genuine second instance or hold the document — a rule with a single instance is fine to
write later and expensive to un-inflate once it has landed. When a reviewer removes one
citation, re-derive the whole list rather than decrementing the number: the reason the first
one didn't fit usually applies to its neighbours.

```markdown
<!-- WRONG — four accurate summaries of four real findings, offered as four
     instances of "a hand-built code block sat next to a citation and was
     mistaken for a quote". Two involve no code block at all: one is a false
     prose claim about a correctly-quoted fixture, one is a miscounted tally of
     YAML entries. The document's own closing paragraph routes both to siblings. -->
Mined from four 🟡 findings in one week …

<!-- RIGHT — the count is what survived the test, and the excluded findings are
     named with the sibling they belong to, so the next miner doesn't re-add them. -->
Mined from one 🟡 finding … The TESTkeyF0rE2eTest and "four earlier allow-rules"
findings are *not* instances of this rule — no block was reconstructed in either;
they belong to `a-resolving-identifier-is-not-a-verified-claim.md` and
`name-the-invariant-not-a-count.md` respectively.
```

Real precedent — three documents, and the flagship case cost three review cycles:

- **`label-a-reconstructed-code-block-as-not-a-quote.md`**, cycles 2 and 3 (the
  `New standard: label-a-reconstructed-code-block-as-not-a-quote` re-reviews in
  `docs/review-logs/2026-09-04.md`; those headings carry no ticket key to cite).
  Cycle 2 (🟡): two of the four cited findings do not instantiate the rule — *"This inflates
  the recurrence claim from 2 genuine instances to 4"* — and the document's own "sibling of
  `name-the-invariant-not-a-count.md`" sentence already said where one of them belonged.
  Cycle 3 (🟡), re-deriving from primary sources rather than trusting cycle 2's verdict, found
  the third citation failed too: the block in question had been programmatically diffed
  against `abc08537^` and confirmed verbatim, so it was never a reconstruction; the defect was
  a prose line-count. *"The evidence base is not 'two, already thin' — it is one"*, with the
  recommendation to hold the document until a second real instance turned up. Cycle 3's own
  note is the rule in one line: *"a citation that resolves and whose content summary is
  accurate in isolation can still be offered as evidence for the wrong thesis."*
- **`a-failure-path-must-emit-what-its-siblings-emit.md`** (🟢, KYO-463's
  `"a failure path must emit what its siblings emit" mined standard` review, `2026-09-03`).
  "Four findings, four tickets, two languages, four days" — the four cited entries land on
  three distinct calendar days, not four. The findings were real and the pattern genuinely
  recurred; the *spread*, which is what makes a recurrence claim persuasive, was overstated.
- **`a-resolving-identifier-is-not-a-verified-claim.md`** (🟡, KYO-533's
  `land six orphaned standards files` review, `2026-08-30`). Blocked signing because the new
  file used the *identical* flagship incident — KYO-429, cycles 1 and 2 — as the primary
  motivating example of the pre-existing sibling
  [verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md).
  One incident cannot be the flagship evidence for two rules without one of them being a
  restatement of the other; the shared evidence was the tell that the rules overlapped.

Sibling of
[verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md)
and
[a-resolving-identifier-is-not-a-verified-claim.md](a-resolving-identifier-is-not-a-verified-claim.md):
those rules are about whether each citation is *true* — the name resolves, the count is the
count, the finding is in that entry. This rule assumes all of that already passed and asks the
next question, which is whether the true citations are evidence for *this* proposition. Every
finding in the flagship case above had an individually accurate summary; the mismatch was
between the set and the thesis it was offered for.

Sibling of
[name-the-invariant-not-a-count.md](name-the-invariant-not-a-count.md): that rule says do not
reach for a tally when a named property carries the argument. Here the tally is the argument —
recurrence is the whole reason a mined rule earns a file — so it cannot be replaced by a
property, only made honest.

See also
[distinguish-the-nearest-sibling-rule-in-the-file-itself.md](distinguish-the-nearest-sibling-rule-in-the-file-itself.md):
working out which sibling an ill-fitting incident actually belongs to is how you write that
disambiguation, and an incident you cannot place in either file is a sign the boundary between
them has not been drawn yet.
