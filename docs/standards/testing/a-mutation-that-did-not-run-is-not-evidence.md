# A mutation that produced no failure is not evidence until you show it reached the code

Mutation testing is the codebase's standard proof that a test is load-bearing:
break the line the assertion depends on, watch the test go red, restore. The
protocol is written down in
[prove-test-fails-without-fix.md](prove-test-fails-without-fix.md), and reviewers
re-run it rather than trusting a report.

It has a blind spot on the green side. "I mutated it and nothing failed" has two
completely different meanings and looks identical in both:

1. **The mutation ran and the suite survived it.** That is a real coverage gap —
   the assertion is vacuous, or the branch is untested, and the fix is a test.
2. **The mutation never executed against the code under test.** The harness read
   a different copy of the file, the migration was skipped by a checksum guard,
   the build did not finish, the fixture pointed somewhere else. That is a
   statement about your setup and says nothing at all about the test.

Case 2 is easy to hit precisely because mutation testing pushes you *out* of the
normal invocation: you copy a file to scratch, point a lint at an override
directory, run one engine of a two-engine contract test, or race a cold build
against a review budget. Each of those detours can quietly sever the link between
the bytes you edited and the bytes the assertion observed — and unlike a broken
red run, a broken green run produces no diagnostic to read.

The cost of confusing them is asymmetric and bad in both directions: reported as
case 1, a healthy test gets rewritten or a spurious 🟡 lands on a clean diff;
assumed to be case 2, a genuine hole is waved away as "the harness."

**Rule:** Before reporting that a mutation did not kill anything, prove the
mutation was in effect at the moment the assertion ran. Establish a *positive
control* — the same run, unmutated, must produce the outcome you expect, on the
same invocation, same paths, same fixtures — or show the mutated bytes in the
artefact the harness actually read. When you detour from the normal invocation,
name the detour and check what it changed: a script copied out of its repo loses
its repo-relative defaults; a shared dev database may refuse a re-applied
migration; a multi-engine test can kill on one engine and never reach the other.
If the run could not complete — build budget, host contention — say the run is
not meaningful and fall back to tracing the code path by hand, rather than
reporting an unfinished run as a result. Only once the mutation is confirmed live
does a surviving suite mean what case 1 means.

```bash
# WRONG — run the pre-fix script from scratch, report the number
$ git show HEAD:scripts/lint/check-disposal-safety.sh > /tmp/old.sh && chmod +x /tmp/old.sh
$ /tmp/old.sh /tmp/fixtures/get_with_arg.rs
#   → no findings.  "The new fixtures don't detect anything."
#
#   The script derives REPO_ROOT from its own location and defaults
#   LINT_DIR="${DISPOSAL_LINT_DIR:-$REPO_ROOT/crates/kyomi-ui/src}", then only
#   accepts an argument matching "$LINT_DIR"/*.rs. From /tmp that pattern
#   matches nothing, TARGETS stays empty, and the fixture was never linted.

# RIGHT — pin the path the detour moved, and keep a positive control
$ DISPOSAL_LINT_DIR=/tmp/fixtures /tmp/old.sh /tmp/fixtures/get_with_arg.rs
#   → WARN:B ...   the pre-fix script really does fire on it
$ DISPOSAL_LINT_DIR=/tmp/fixtures \
    scripts/lint/check-disposal-safety.sh /tmp/fixtures/get_with_arg.rs
#   → clean.  Same invocation, same fixture, only the script differs:
#     now "7 of 9 fixtures fail against the pre-fix script" means something.
```

Four instances in four days, across three unrelated harness families:

- **KYO-414** (review log `2026-08-24`, `03:27`) — the canonical case. The
  reviewer's first pass "gave a false `0 failures` reading because copying the
  old script out to scratch broke its `REPO_ROOT`-relative `LINT_DIR` default";
  overriding `DISPOSAL_LINT_DIR` explicitly turned the same mutation into **7 of
  9 fixtures failing**. The entry's own note is the rule in one line: copying a
  script out of its own repo tree "silently breaks its own `REPO_ROOT`-relative
  defaults, which nearly produced a false `no real detection` finding here."
- **KYO-460** (review log `2026-08-23`, `14:00` initial and `18:30` cycle 2) —
  a two-engine contract test where one engine killed and the other never ran the
  mutated SQL: SQLite "failed exactly as predicted … `left: String("true") right:
  Bool(true)`", while "Postgres hit the (correct, harmless) sqlx
  migration-checksum guard instead, because this exact migration was already
  applied to the shared dev Postgres before this review started — not a defect,
  the harness's own safety net." Reported as a harness artefact, not as
  Postgres-side weakness — which is only defensible because the discriminator was
  identified.
- **KYO-480** (review log `2026-08-23`, `22:09`, cycle 2) — the incomplete-run
  case, handled correctly: the mutated file was restored before the build reached
  the crate under test, "so the in-flight run is not meaningful and was not waited
  on further"; the reviewer fell back to hand-tracing a deterministic pure
  function and said so, rather than quoting the aborted run.
- **KYO-314** (review log `2026-08-21`, `12:05`) — same shape on a shared box:
  "the rebuild ran into heavy contention from concurrent builds elsewhere … and
  did not finish before the code-path tracing above already gave a dispositive
  answer." Mutation reverted, no result claimed from it.

And the contrast that makes the distinction worth drawing — **KYO-444** (review
log `2026-08-22`, `08:10`, 🟡): collapsing `ConfiguredProjectScope`'s two
`Explicit` arms into "a single unconditional `Explicit(projects) => projects`
still passes all 10 existing tests." Identical surface observation, opposite
meaning: the mutation genuinely ran, the guard clause genuinely had no coverage,
and the review would not sign without a test. That is "the original bug wearing a
new hat," and it is exactly what a false case-2 reading would have buried.

Mirror image of
[anchor-source-text-markers-on-code-not-copy.md](anchor-source-text-markers-on-code-not-copy.md),
which covers the red-for-the-wrong-reason half — a `marker not found` panic under
mutation means the anchor is wrong, not that the defect was caught. This rule is
the green-for-the-wrong-reason half. Kin to
[skipped-test-must-fail-loudly.md](skipped-test-must-fail-loudly.md): there a
*test* silently declines to run and the total still looks healthy; here a
*mutation* silently declines to apply and the suite still looks meaningful.
