# Verify a precedent claim against the source it cites

Standards rules earn their authority from the incidents they cite. "This cost two rework
cycles on KYO-N" is what makes a reader follow the rule instead of arguing with it. That
makes the incident summary the *load-bearing* part of the document — and it is also the
part most likely to be wrong, because it is the part written from memory. The author has
the code open; they do not have the review log, the merged diff, or the precedent ticket
open, so they reconstruct the story and the reconstruction drifts.

Nothing catches the drift. A misquoted signal name in a precedent paragraph is not a code
example, so [verify-every-identifier-in-a-doc-code-example.md](verify-every-identifier-in-a-doc-code-example.md)
does not cover it; it is not a `file:line` anchor, so
[anchor-a-citation-to-a-symbol-not-a-line-number.md](anchor-a-citation-to-a-symbol-not-a-line-number.md)
does not either. It is a claim about history, and the only thing that can check it is
opening the history.

The failure is worse than an ordinary inaccuracy, because a wrong precedent teaches the
wrong lesson with full confidence. A reader who greps for the misquoted name finds nothing
and concludes the rule describes code that no longer exists — so the rule gets ignored
rather than corrected.

**Rule:** Before committing any documentation that cites a review log, a merged commit, a
ticket, or a past incident, open the thing you cited and check the claim against it —
every name it quotes, every count it asserts, every date it gives. Re-derive counts from
the diff rather than copying the log's summary of them; the log can be wrong too. If you
cannot open the source, do not make the claim: describe the shape of the failure without
attributing it.

```markdown
<!-- WRONG — written from memory of the incident. The signal is `test_result`;
     `connection_test_result` appears in this codebase only inside unrelated
     badge-component test names. And the sweep was five sites, not four. -->
Validating under `kyomi_oauth` then switching mode left
`connection_test_result.success == true` and held the gate open.
Removing the `discovery_error` signal took four sites for one signal.

<!-- RIGHT — each name grepped, the count re-derived from commit 4524bcc8's diff
     (2 setters + 2 resets + 1 reader) rather than copied from the log's prose. -->
Validating under `kyomi_oauth` then switching mode left
`test_result.success == true` and held the gate open.
Removing the `discovery_error` signal took five sites for one signal.
```

Mechanical check before staging a docs change that cites history:

```sh
grep -rn "test_result" crates/            # every quoted identifier resolves
git show 4524bcc8 -- <path> | grep -c …   # every asserted count re-derived
sed -n '/13:05/,/^### /p' docs/review-logs/2026-08-23.md   # every quoted log entry read
```

Mined from four findings across two tickets in one week, every one a documentation file
misdescribing an incident it cited:

- **KYO-433** (`2026-08-23`, 12:27) — two 🟡 in
  `docs/standards/data-state-management/teardown-clears-the-whole-derived-state-group.md`:
  a precedent paragraph quoting a signal that does not exist under that name, and a "four
  sites" count that the cited log *and* the merged diff both put at five.
- **KYO-429** (`2026-08-22`, 05:06 and 05:45) — the same citation paragraph flagged 🟡 in
  two consecutive cycles: first for three `file:line` incident locations that corresponded
  to no real content, then, after the rewrite, for naming the wrong logs and framing three
  separate findings as one review pass.

The KYO-429 pair is the point. Both cycles were spent on the citation attached to a rule
about unverifiable claims, in a section whose whole subject is keeping documentation
claims true. Writing the rule does not exempt its own evidence from it.
