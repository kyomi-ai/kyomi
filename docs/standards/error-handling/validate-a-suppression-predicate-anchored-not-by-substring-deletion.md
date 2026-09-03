# Authorize a skip only from a whole-predicate match — never from a text check a fragment can satisfy

A gate that decides *whether to skip a check* is the one place where a permissive
parse is not a cosmetic bug. Every other parser that gets a shape wrong reports
something wrong; a suppression predicate that gets a shape wrong reports **nothing
at all**, and a silent zero-findings run is indistinguishable from a clean one.

The recurring mistake is asking a *textual* question ("does this predicate contain
`test`?") in place of a *structural* one ("is this predicate exactly a form I model
and have decided is test-only?"). Text has no notion of scope or polarity, so a token
nested inside a larger expression that means something different — or even the
opposite — satisfies the check just as happily as a top-level one.

`scripts/lint/check-disposal-safety.sh` hit this twice in one day, in two consecutive
review cycles. The two are worth reading together, because they are **different
mechanisms reaching the same failure**, and the second was introduced by the fix for
the first:

- **Cycle 1 (`:458`) — containment.** `cfg_predicate_is_test()` treated any compound
  predicate merely *containing* the bare token `test` as test-exclusive. No deletion,
  no rewriting: a plain "does this token appear anywhere" match. So
  `#[cfg(not(all(test, feature = "x")))]` — true in nearly every production build —
  armed the skip, silently swallowing a Rule A finding that fires correctly on
  pristine `origin/main`.
- **Cycle 2 (`:511`) — deletion, then flatness measured on the residue.** The fix added
  an unanchored `gsub` that stripped `not(test)` from anywhere in the predicate string
  *before* the flat-shape check ran. For the tautology `#[cfg(any(not(test), test))]`
  — `(NOT test) OR test`, true in **every** build — the substitution left `any( , test)`,
  which contains no nested parens, so the flat-shape regex accepted it and declared the
  module test-only. A module gated by exactly that predicate, containing a live Rule A
  trigger, produced **zero** findings.

Both were 🔴, and both disabled a *blocking* rule on code that ships to production.
Note the direction of the second failure: the docstring claimed the function "fails
closed on anything not a flat shape", and that claim was false precisely because
flatness was measured on the mutated string rather than on the input.

**Rule:** a predicate that authorizes skipping a check must be matched, whole and
anchored (`^…$`), against the original unmodified text. Do not decide from
containment, and do not normalize by deleting sub-terms first. If the input contains
any structure the matcher does not explicitly model — a nested `(`, an extra operand,
a negation, an unknown token — **do not skip**: fall through and run the check. Every
unrecognized shape must resolve to *more* checking, never less. Recognize or decline;
never repair.

The code below is a **reconstruction** of the two shapes, not a verbatim quote of
either revision — the real function also does whitespace trimming and token-boundary
matching. It is written to isolate the defect.

**WRONG (cycle 1)** — containment; polarity and nesting are invisible:

```awk
function cfg_predicate_is_test(pred) {
    return (pred ~ /test/)        # not(all(test, ...)) matches too
}
```

**WRONG (cycle 2)** — deletes a known sub-term, then measures flatness on the residue,
so a tautology launders itself into a skip:

```awk
function cfg_predicate_is_test(pred) {
    gsub(/not\([[:space:]]*test[[:space:]]*\)/, "", pred)   # unanchored: strips anywhere
    if (pred !~ /\(/ && pred ~ /test/) return 1             # flatness measured on residue
    return 0
}
```

**RIGHT** — recognize the whole predicate, or decline to skip:

```awk
function cfg_predicate_is_test(pred) {
    if (pred ~ /^[[:space:]]*test[[:space:]]*$/) return 1
    if (pred ~ /^[[:space:]]*not[[:space:]]*\([[:space:]]*test[[:space:]]*\)[[:space:]]*$/) return 0
    return 0        # anything else, including any nesting we do not model: run the check
}
```

Real precedent: KYO-558 / KYO-612 (2026-09-03), `scripts/lint/check-disposal-safety.sh:458`
and `:511` — 🔴 both times, the second reintroduced by the first's fix. See
`docs/review-logs/2026-09-03.md`.

See also `empty-on-failure-must-not-look-like-a-real-result.md` in this section (a failed
check must not be readable as a passing one) and
`a-guard-in-one-branch-does-not-cover-the-others.md` (a guard's coverage is what it
actually matches, not what it was written to mean). This rule is the narrower case where
the guard's own *input* was never structurally inspected at all.
