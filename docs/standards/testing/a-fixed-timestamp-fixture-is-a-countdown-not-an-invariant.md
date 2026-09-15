# A fixed timestamp fixture is a countdown, not an invariant

A test fixture carrying a hardcoded absolute date, used against code that filters on a window
relative to *now*, is not a constant. It is a timer. Every hour that passes moves it closer to
the cutoff, in one direction only, and it never moves back. The comment next to it will say
something reassuring — "safe for a long time", "well within the window" — and that comment is a
prediction with an expiry date, not a property of the code.

The failure has three qualities that make it unusually expensive:

- **It arrives from nowhere.** Nothing in the diff that goes red touched the test, the fixture,
  or the code under test. The first person to meet it is whoever happened to open a PR that
  morning, and their reasonable first hypothesis is that they broke something.
- **Re-running does not help.** Three consecutive runs give three identical failures, so the
  usual flake triage returns "reproducible, therefore real" and points the investigation at the
  diff.
- **It only gets worse.** Unlike a race, it has no chance of passing again. The day after it
  starts failing, it fails harder.

**Rule:** If a test's verdict depends on how far a fixture's timestamp is from *now*, compute
that timestamp from the clock at test time. Reserve hardcoded absolute dates for fixtures whose
correctness the passage of time can only *reinforce*.

That asymmetry is the whole rule, and it is worth stating explicitly in the code, because the
two cases sit side by side and look identical:

- A fixture meant to be **inside** a relative window must be computed. Time moves it out.
- A fixture meant to be **outside** every realistic window may be a literal. Time moves it
  further out — the claim it embodies only becomes more true.

Pick the computed offset with margin at both ends. It must stay in the past (a future
`mergedAt` on a merged PR is nonsense), and it must sit far enough inside the window that clock
skew, a slow suite, or a CI runner queued behind something else cannot push it over the edge.

Two non-fixes to refuse:

1. **Bumping the literal to a more recent date.** This is the one that gets proposed, because it
   turns the suite green in one line. It reintroduces the identical bomb with a later fuse, and
   it will go off on a day when nobody remembers this happened before.
2. **Passing an explicit wide window everywhere** so the fixture date stops mattering. This does
   turn the suite green, and it deletes the coverage that matters: the default is the value
   production actually runs with, so the default path is the one most worth testing. Fix the
   fixture, not the call.

```bash
# WRONG — quoted verbatim from scripts/reconcile-merged-tickets-test.sh before
# KYO-792. The script under test defaults to --lookback-hours 336 (14 days) and
# filters on a cutoff computed from `now`, so this literal was never "within
# 336h for a long time" — it was within 336h for exactly 336 hours.
FUTUREPROOF_TS='2026-09-01T00:00:00Z' # within 336h (14d) of "now" for a long time

# RIGHT — computed from the clock, with the margin stated and the asymmetry
# against ANCIENT_TS made explicit so a later reader does not "simplify" one
# into the other.
FUTUREPROOF_TS="$(date -u -d '-7 days' +%Y-%m-%dT%H:%M:%SZ)" # inside the default 336h window, 168h margin either side
ANCIENT_TS='2015-01-01T00:00:00Z'  # literal ON PURPOSE: outside ANY realistic lookback, and time only pushes it further out
```

Prove the claim rather than asserting it. `faketime` and `datefudge` are not installed on this
box, but a `date` shim placed first on `PATH` that forwards to `/usr/bin/date` with the
reference time shifted forward a year is enough, and it is honest: both the suite and the script
under test read the clock through it, so they shift together. Run the suite under the shim and
confirm the tally is unchanged. Keep the shim out of the repo — it is a verification tool, not a
deliverable.

Real precedent — **KYO-792**. `scripts/reconcile-merged-tickets-test.sh` pinned every fixture's
`mergedAt` to `2026-09-01T00:00:00Z`. On 2026-09-15 the default 336h cutoff overtook it by a
matter of hours: in the CI run that first went red (PR #533, workflow run `34929453631`), the
suite started at `2026-09-15T04:36:16Z`, putting the cutoff at `2026-09-01T04:36:16Z` — four
hours and thirty-six minutes past the fixture. Every fixture using the constant was filtered out
before extraction ran, so **19 of the suite's 99 assertions returned zero rows** and the
**Worktree Lifecycle & Script Self-Tests** CI job went red on *every* PR in the repository —
with `scripts/reconcile-merged-tickets.sh` itself entirely correct and unmodified. The
green-to-red transition is visible in CI and was confirmed PR-by-PR: #526, #529 and #530 all
show that job passing on 2026-09-13/14, and #533 shows it failing on 2026-09-15. Nothing about
the code changed in between; only the date did.

Note how little warning that margin represents. The suite was not "nearly out of window" for
days in a way anyone could have noticed — it was inside on one CI run and outside on the next.
A countdown gives no signal until it reaches zero, which is why the defect has to be designed
out rather than watched for.

The second-order cost is the one to weigh when deciding whether this is worth a rule.
`reconcile-merged-tickets.sh` is `/merge-sweeper`'s merged-PR reconciliation pass, and this
suite is the only thing asserting its `Closes KYO-NN` extraction works. A suite that is red for
an unrelated reason is a suite nobody reads, so the next genuine extraction regression would
have landed invisibly behind the noise — see
[nondeterministic-verdict-is-a-failing-test.md](nondeterministic-verdict-is-a-failing-test.md)
for the same argument about a differently-broken verdict.

Distinct from
[a-tests-verdict-must-not-depend-on-the-ambient-environment.md](a-tests-verdict-must-not-depend-on-the-ambient-environment.md):
that rule covers an input that varies **across machines** at one instant, and its check is to
run the suite in a different shell. That check will never catch this one — every machine agrees,
and the divergence is **across time on all of them at once**. Distinct from
[nondeterministic-verdict-is-a-failing-test.md](nondeterministic-verdict-is-a-failing-test.md):
there the verdict flips between runs and three consecutive runs expose it; here three
consecutive runs agree perfectly, both before the cutoff is crossed and after. The three rules
partition cleanly by what you would have to vary to observe the bug — the shell, the run, or the
date — which is also the order in which a tired reviewer will fail to think of them.
