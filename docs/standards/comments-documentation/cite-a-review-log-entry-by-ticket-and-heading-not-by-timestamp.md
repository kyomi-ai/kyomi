# Cite a review-log entry by ticket and heading, not by timestamp

`docs/review-logs/` is the corpus every standards rule draws its authority from, so almost
every mined rule ends with a paragraph pointing back into it. The obvious way to write that
pointer is the coordinate the log appears to offer: a date (the filename) and a time (the
`### HH:MM —` heading). Both parts are unreliable, and they are unreliable in ways that do
not announce themselves — the file exists, the timestamp exists, and the reader who follows
the pointer lands on a real review entry that simply isn't yours.

The date is the **append** date, not the event date. `scripts/append-review-log.sh:95` writes
to `${logs_dir}/$(date +%F).md`, evaluated when the append runs, so a review that concluded
late on one day and was written up after midnight is filed under the next day. Citing "the
`2026-09-02` log" for work you remember doing on 2026-09-02 is a coin flip.

The time is free text the reviewing agent types into its own heading. Nothing derives it,
nothing orders it, and nothing enforces it:

- **Not monotonic.** `2026-09-03.md` opens `14:20`, `01:10`, `09:40`, `03:53`, then an
  untimed entry, then `04:32`. Scanning to "the 20:35 entry" by reading downwards finds the
  wrong one.
- **Not unique.** `### 20:35` appears twice in `2026-08-30.md` (lines 500 and 516 — the
  initial and cycle-2 reviews of the same file). `### 14:20` appears twice in `2026-09-03.md`,
  on two different tickets.
- **Sometimes absent.** `2026-09-03.md:138` is `### (session) — Two new coding standards: …`.
  There is no timestamp to cite; a paragraph that cites only timestamps silently drops that
  entry from its evidence, or invents one. The ticket key can be missing too — the three
  `### … — New standard: label-a-reconstructed-code-block-as-not-a-quote` entries in
  `2026-09-04.md` carry no `KYO-NN` anywhere in their headings, and a citation that supplies
  one anyway has fabricated it.
- **Not a reliable `sed` range, either.** Entries contain their own `###` sub-headings —
  `### Minor Issues (🟢 — logged, not blocking)` and `### Positive Observations` sit *inside*
  the 22:17 entry at `2026-09-03.md:1021`/`:1026` — so `sed -n '/22:17/,/^### /p'` stops
  before that entry's findings and reports them as absent.

What *is* stable is the ticket key and the heading's own text. Those are chosen by the
author, carried verbatim into the heading, and greppable across the whole directory without
knowing which file the entry landed in.

**Rule:** Anchor a review-log citation on the ticket key plus enough of the heading text to
be unambiguous — "KYO-629's `preflight-clippy.sh + CI parity self-test` review" — and find it
with `grep -rn "KYO-629" docs/review-logs/`, which is indifferent to which day's file it was
appended under. If you also give a date or a timestamp, treat it as a convenience label you
have just confirmed by opening the file, never as the thing that locates the entry: search
the neighbouring day's file too before concluding an entry is missing. Quote the heading
rather than the clock when the entry has no timestamp, and quote it *alone* when the heading
carries no ticket key — supplying a key the heading does not contain is a fabricated citation,
not a helpful one. Verifying that the entry you
landed on actually contains the finding you attribute to it is a separate obligation, covered
by the two siblings named at the end of this file.

```markdown
<!-- WRONG — a date/time coordinate, presented as the address of the finding.
     The 20:35 entry in that file is a wholly clean KYO-629 rework review with no
     such finding; the finding is at 16:40. A second cited finding lives in an
     untimed `### (session) —` entry, so no timestamp could have addressed it. -->
Mined from the 2026-09-03 log (12:35 / 13:40 cycles, and 20:35).

<!-- RIGHT — ticket + heading text, each confirmed by grepping the directory,
     with the day named only after the file it actually landed in was opened. -->
Mined from KYO-629's `preflight-clippy.sh + CI parity self-test` review and from
the untimed `(session)` review of the two `guard-fixed-for-one-branch` /
`prove-a-flagged-secret-is-fake` standards (both `docs/review-logs/2026-09-03.md`).
```

```sh
# Locate the entry without assuming a day, then read from its heading to the
# *next entry's* heading rather than the next `###` of any kind.
grep -rn "KYO-629" docs/review-logs/
awk '/^### .*KYO-629: preflight-clippy/{p=1} p&&/^---$/{exit} p' \
  docs/review-logs/2026-09-03.md
```

Three documents in four days, each pointing at a real entry that was not the one it meant:

- **`label-a-reconstructed-code-block-as-not-a-quote.md`** (blocked signing 🟡; the
  `New standard: label-a-reconstructed-code-block-as-not-a-quote` review — one of the
  ticketless headings this rule warns about — in `docs/review-logs/2026-09-04.md`).
  Cited "the 2026-09-03 log (12:35 / 13:40 cycles, and 20:35)" for three of four findings.
  The real 20:35 entry (`2026-09-03.md:860`) is a KYO-629 rework with a Clean verdict and no
  technical-accuracy finding in it whatsoever; the intended finding is at 16:40
  (`:742-753`). A second cited finding has no timestamp at all — it is in the untimed
  `### (session) —` entry at `:138`, which the paragraph never mentioned. Only the 12:35 half
  resolved. The cycle-2 fix cited the untimed entry by *description* rather than by clock,
  which is the shape this rule prescribes.
- **`validate-a-suppression-predicate-anchored-not-by-substring-deletion.md:64`** (🟢, KYO-629's
  `preflight-clippy.sh + CI parity self-test` review, `2026-09-03`). "Real precedent: KYO-558 /
  KYO-612 (2026-09-02)" — the cited entries are in `2026-09-03.md`, the file they were
  *appended* to. Off by one day; fixed by rewriting the date, per the same log's rework entry.
- **`verify-the-replacement-before-destroying-the-original.md`** (🟢, KYO-463's
  `two new coding-standard docs (spec-green mining pass)` review, `2026-09-01`). Quoted
  KYO-567's fast-forward/non-ancestor-sha reasoning correctly but attributed it to "Cycle 3";
  the reasoning is in the `16:51` entry, which the log labels cycle 2. The `16:57` entry —
  the actual cycle 3 — covers different content. Ticket, date and quote were all correct; only
  the coordinate was wrong.

Sibling of
[anchor-a-citation-to-a-symbol-not-a-line-number.md](anchor-a-citation-to-a-symbol-not-a-line-number.md):
that rule is the same move for source files — cite the symbol, not `datasources.rs:5059`,
because line numbers drift as the file is edited. This rule is its review-log counterpart,
and the failure is not drift: a log entry is append-only and never moves. The coordinate was
never trustworthy in the first place, because the filename records when someone typed the
entry and the timestamp is prose.

Sibling of
[a-resolving-identifier-is-not-a-verified-claim.md](a-resolving-identifier-is-not-a-verified-claim.md)
and
[verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md):
both are about *checking* a citation — open what you cited and confirm it says what you say it
says. They assume you can address the entry; this rule is about the address itself, and it is
why their own recommended `sed -n '/13:05/,/^### /p'` recipe can come back empty (wrong day's
file) or truncated (an entry's internal `###` sub-heading ends the range early) on a log entry
that is really there.
