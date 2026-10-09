# Quote the artefact, not the log that quotes it

Almost every rule in this corpus is mined out of `docs/review-logs/`, and a review-log
finding is a **composite document**. One sentence routinely carries two kinds of text at
once: an excerpt the reviewer quoted out of some file, and the reviewer's own commentary
about that excerpt — its ticket status, its line number, what it proves. Both sit inside
the same sentence, often inside the same pair of quotation marks, because the reviewer was
writing for a reader who had just read the diff.

Copy a span out of that sentence and the words survive but the seam does not. What lands in
the standards file is a quotation attributed to the artefact — and a quotation carries an
implicit claim that nothing else in a citation carries: *these are that artefact's own
bytes*. That claim has exactly one check, and it is a one-liner the miner did not run.

Two shapes recur, and they fail the same check:

- **Wrong speaker.** The quoted span is the reviewer's prose, not the file's. It is verbatim
  present in the log, so
  [verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md)
  passes cleanly — the source cited was the log, and the log does say it. The file the quote
  is *attributed* to has never contained that string in its entire history.
- **Wrong subject.** The quoted span really is the file's, at the line given, byte-for-byte —
  but the log's surrounding sentence supplied what it demonstrated, and the mined rule
  re-attached it to a different claim. Real quote, real `file:line`, wrong proposition.

A third, milder form breaks the same check from the other end: a span introduced as
"verbatim" that has been re-marked in transit — backticks or italics added around an
identifier, bold normalised away — so the words are unchanged and `grep -F` still finds
nothing.

Every one of these reads as the best-sourced paragraph in the document. The line number
resolves, the ticket exists, the log entry is real, and the only reader who discovers
otherwise is the one who does the thing the rule told them to do: opens the named file and
looks for the string.

**Rule:** Before a quoted span ships in a standards file, `grep -F` that exact span in the
artefact you are attributing it to — not in the log you found it in. If it is not there,
it is not a quote: paraphrase it outside the quotation marks and name who actually said it
("the reviewer's own note that…"). If it *is* there, re-read the surrounding lines of the
artefact and confirm they support the claim your sentence attaches to them; the log's
framing is not part of the file. If a span is introduced as verbatim, it must byte-match —
re-mark nothing, and mark every cut with an explicit elision rather than silently
normalising emphasis.

```markdown
<!-- WRONG — reconstructed from the reviewer's own excerpt; the file itself was
     deleted from the commit rather than patched a fourth time, so no `<sha>^`
     survives to quote. The parenthetical is the reviewer's aside about ticket
     status in a 2026-09-09 log entry, presented here in this document's own
     verbatim-quote convention as wording build-test.md carries. It does not,
     and never has. -->
… which notes that `kyomi-private/skills/build-test.md` still carries the
pre-fix wording *"(KYO-687 is In Review, unmerged)"*

<!-- RIGHT — the quoted span is what the file actually carries; the status is
     stated as the document's own prose, outside the quotation marks. The log
     sentence this was mined from is quoted verbatim below it for contrast. -->
`kyomi-private/skills/build-test.md` still carries the pre-fix wording
*"Pass `run_in_background: false` on every `Agent` call"*. That ticket was in
review and unmerged when the finding was written.
```

The log sentence both blocks were mined from, quoted verbatim from the **KYO-682** entry
`panic reports ship an empty console_errors array` (re-review cycle 2, `2026-09-09` log) —
note that it contains a file quote and a reviewer aside back to back:

> `kyomi-private/skills/build-test.md:256` still reads "Pass **`run_in_background: false`**
> on every `Agent` call" (KYO-687 is In Review, unmerged)

Mechanical check before staging any standards file that quotes an artefact:

```sh
# Is this string actually in the file you are attributing it to — ever?
grep -Fn 'Pass `run_in_background: false` on every `Agent` call' path/to/artefact
git log --all -S'(KYO-687 is In Review, unmerged)' -- path/to/artefact   # zero hits ⇒ not a quote

# Is it attached to the subject you claim? Read around it, not just at it.
grep -Fn -B3 -A3 'either direction' path/to/artefact
```

Real precedent — one file, two consecutive blocking cycles, and it was ultimately deleted
rather than fixed:

- **KYO-705**, `2026-09-10` log, the `Standards-mining commit (KYO-705 branch): three new
  coding-standard rules (re-review)` entry — 🟡, blocked signing. The wrong-speaker shape,
  verbatim. The document presented `"(KYO-687 is In Review, unmerged)"` in its own
  italic-plus-quotes convention as wording `kyomi-private/skills/build-test.md` carries.
  The reviewer ran `git log --all -S"unmerged"` and `-S"In Review"` against that file and
  got **zero hits across its entire history** — the string is the reviewer's own
  parenthetical aside in the KYO-682 entry above; the wording the file carries is the
  `run_in_background: false` line. The reviewer's own summary of the cost: *"A reader who
  opens `build-test.md` looking for '(KYO-687 is In Review, unmerged)' — exactly the
  verification step this document and its cited sibling
  `verify-a-precedent-claim-against-its-source.md` prescribe — will not find it there."*
- **Same file, same ticket, the `… (re-review, cycle 3)` entry** — 🟡, blocked signing again.
  The wrong-subject shape. Three passages cited at `build-test.md:266-267`,
  `backlog-fast/SKILL.md:583` and `backlog/SKILL.md:514` are, in the reviewer's words,
  *"verified, verbatim-accurate quotes, but they are about a different hazard entirely"* —
  they disambiguate the Bash tool's real `run_in_background` parameter from the Agent tool's
  non-existent one, not the "takes no side" disclaimer the paragraph claimed they guarded.
  `grep -n "either direction"` finds the phrase exactly once in that file, attached to the
  Bash-tool sentence. Verdict: *"Same defect class as cycles 1 and 2: real quotes, real line
  numbers, wrong claim about what they demonstrate."*
- **Same ticket, the `… (re-review, cycle 4)` entry** — the file was dropped from the commit
  entirely rather than patched a fourth time, and its content filed as **KYO-738**. The
  reviewer's accounting: *"Four review cycles on this diff, three of them against a defect
  class (real quotes attached to claims they don't support) confined entirely to the file
  that's now gone."* Two surviving files in the same commit never carried the defect. That
  is the cost of the check nobody ran: not a wrong sentence, a discarded rule.
- **KYO-692 / KYO-713**, `2026-09-11` log, the `rename and correct the headless sub-agent
  standard (PR #505, cycle 2)` entry — 🟢, the re-marking form. A passage introduced as
  "verbatim" injected markdown backticks around `--max-budget-usd` and `-p` that the harness
  binary's own string does not carry, while an adjacent "verbatim" passage left the same
  class of identifier unmarked. Resolved in cycle 3 by dropping them: *"both 'verbatim'
  passages now byte-match the binary."*

The safe direction, for calibration — **KYO-682**, `2026-09-09` log, the initial
`panic reports ship an empty console_errors array` entry, 🟢: a WRONG block composited two
near-verbatim lines from two different sources, and was *not* presented as a quote. The
reviewer recorded it as *"the safe direction of the reconstruction hazard: near-verbatim
text presented as illustration understates its provenance rather than overstating it."*
Under-claiming provenance costs a parenthetical; over-claiming it costs review cycles.

One nearby finding is deliberately **not** counted here, per
[count-only-the-incidents-that-instantiate-the-rule.md](count-only-the-incidents-that-instantiate-the-rule.md).
The KYO-686 pre-work entry `Two new coding standards: resolve-a-file-claim-in-the-tree-under-review
+ name-the-check-you-could-not-run` (`2026-09-09` log) logged a 🟢 for italics added to the
word `total` inside a quoted excerpt — but the later review of
[a-correct-conclusion-does-not-vouch-for-its-supporting-fact.md](a-correct-conclusion-does-not-vouch-for-its-supporting-fact.md)
(`2026-09-10` log) opened the primed source and found it *does* contain `*total*`. The
original finding was itself wrong. It is worth naming precisely because the retraction came
from running this rule's own check against the artefact instead of trusting the log.

Nearest sibling is
[verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md):
that rule asks whether the claim matches the source you cited, and in every instance above
it *passed* — the log was cited correctly and says exactly what the miner reported. The gap
is that a review log is a secondary source about some other artefact, so satisfying that
rule leaves the quotation's real claim — "these bytes are in that file" — untested. Sibling
of [a-resolving-identifier-is-not-a-verified-claim.md](a-resolving-identifier-is-not-a-verified-claim.md),
which is the general form: there the passing grep is an existence check on a *name* and the
unverified part is the proposition about it; here the passing check is on the *log* and the
unverified part is attribution. Distinct from
[quote-a-wrong-block-from-the-pre-fix-commit.md](quote-a-wrong-block-from-the-pre-fix-commit.md):
that rule is about a block **retyped from memory** because the code it depicts is gone, and
its remedy is `git show <fix-sha>^:<path>`. Nothing here was retyped — the spans were copied
accurately out of a document that does contain them, which is why that rule's reconstruction
test does not fire. Distinct from
[anchor-a-citation-to-a-symbol-not-a-line-number.md](anchor-a-citation-to-a-symbol-not-a-line-number.md)
and [cite-a-review-log-entry-by-ticket-and-heading-not-by-timestamp.md](cite-a-review-log-entry-by-ticket-and-heading-not-by-timestamp.md),
which govern whether a *pointer* still resolves; these quotes' pointers all resolved, and the
reader who followed them is precisely who found the defect. And distinct from
[verify-every-identifier-in-a-doc-code-example.md](verify-every-identifier-in-a-doc-code-example.md),
whose remedy — grep the corpus for the name — returns a false negative here, because every
name in these quotes is real and resolves in several files at once.
