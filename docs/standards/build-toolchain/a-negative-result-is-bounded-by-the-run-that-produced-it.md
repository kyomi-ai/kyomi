# A negative result is bounded by the run that produced it — write the bound, not an all-clear

The verification claims that go unchecked are the ones that came back clean. *It did not
reproduce.* That is a real observation, and it is true only inside the window the run could
see — a duration, a sample, a build, one box's configuration. Drop the window and the
sentence quietly changes meaning: from "not observed under these conditions" to "does not
happen". That second meaning is what a later reader acts on, and nobody re-derives it,
because a negative reads as settled and leaves nothing to go and look at.

Two shapes recur, both in the same window:

- **A short run generalised past its own duration.** A ~150-second probe found no kill and
  the write-up concluded the mechanism "is now historical" — against six recorded failures
  that had all occurred 10-16 minutes in. The sample could not reach the durations that
  failed.
- **A correlation inside the sample promoted to the mechanism.** Three schema observations
  sorted perfectly by session type, so the doc keyed its rule on session type — while the
  tool's own docs key the behaviour on two configuration gates. A rule keyed on the
  correlate breaks the first time the gates and the correlate disagree.

**Rule:** when you record a negative result, record what bounded it in the same breath — the
duration or size the run reached, and the version or build it was taken on (name the
version; "the current harness" is stale the next time the box updates). If the failure you
were probing for is known to occur outside that bound, say it is *unmeasured there*, not
cleared. And do not promote whatever the sample happened to correlate with into the
mechanism: if the tool documents the real gate, quote it; otherwise report the correlation
as a correlation. A banner verdict — a header line, a section title, a PR summary — carries
the bound too, or it will be read alone.

**WRONG** — `scripts/repro-headless-subagent-survival.sh` at `956dd5b4` (KYO-688, PR #503),
lines 53-58 verbatim; the paragraph continues past the quoted range:

```sh
# CONCLUSION AS OF THAT DATE: the KYO-546 failure mode (a background
# sub-agent silently killed by the parent's end_turn, reported in
# subagent_stats.killed.system) did NOT reproduce on harness 2.1.258. It was
# real when scripts/audit-agent-run-deaths.sh was written (KYO-546, harness
# versions predating this one) and appears to have been fixed upstream
# since. This script does not assert that background dispatch is now SAFE
```

**RIGHT** — the same passage in the same file on `main` today, after three review cycles;
the first four lines above are unchanged, so this quotes lines 64-70, from `# versions`
onward. What was added is the word *bound*, the ceiling the tool still documents, and the
gap between the sample's duration and the failures' durations:

```sh
# versions predating this one). What these runs establish is a BOUND, not
# an all-clear: the harness still documents a `-p` wait ceiling for
# background sub-agents (2.1.267's own subagent_stats.killed field text,
# identical in 2.1.245 and in 2.1.258, names "-p giving up on a background
# subagent still running at its wait ceiling"), and a ~150-second sub-agent
# is far shorter than the 10-16 minute KYO-468 deaths that motivated the
# audit. This script does not assert that background dispatch is now SAFE
```

Real precedent — one ticket, three review cycles, three artefacts, and the same root every
time:

- **KYO-692 / KYO-713**, `2026-09-11`, heading *"KYO-692 (+KYO-713) rename and correct the
  headless sub-agent standard (PR #505, rework 2)"* — 🟡: *"That specific kill mechanism is
  now historical — it does not reproduce on the current harness"* generalises "a single
  ~150 s sample", against a `killed.system` field doc that is byte-identical in 2.1.245,
  2.1.258 and 2.1.267 and still names the wait ceiling. The verdict: *"Correct observation,
  wrong inference."* A 🟢 in the same cycle is the same shape again: "the current
  harness" naming no build while the box had moved from the measured 2.1.258 to 2.1.267.
- **Same PR, rework 2, third 🟡** — the correlation shape: the file's mode-dependency table
  *"attributes `run_in_background`'s presence in the Agent schema to session type … from n=2
  observations"*, where the harness documents the gate as *"background tasks disabled, or the
  fork gate on"*. What landed on `main` keeps the observations and drops the causal claim:
  *"these three happen to sort by session type, but that is a correlation this box's
  configuration produces, not the mechanism the harness documents, so a rule keyed on session
  type would be keyed on the wrong thing"*
  ([../agent-orchestration/a-sub-agent-in-flight-must-not-outlive-its-turn.md](../agent-orchestration/a-sub-agent-in-flight-must-not-outlive-its-turn.md)).
- **Same PR, cycle 2** — 🟡: *"appears to have been fixed upstream since"* was still standing
  in **both** tracked scripts, *"the exact inference cycle 1 🟡2 ruled unsafe"*, and the
  standard names one of them as the post-upgrade re-check entry point — so fixing the
  conclusion in the prose left it live in the tooling that outlives the prose.
- **Same PR, cycle 3** — 🟢, the banner case: `KYO-688 RE-TEST … — DID NOT REPRODUCE` sat
  ~50 lines above the paragraph that bounds it; the suggested repair was a trailing
  *"WITHIN A ~150s BOUND"* on the header itself.

The habit done right, in an unrelated domain, the day before: **KYO-710**, `2026-09-10`,
heading *"KYO-710 cron auth-failure detection (re-review, cycle 2)"* — a signal was argued
redundant by checking "all six real incident logs", and the conclusion was scoped to exactly
that sample: removing it *"would not lose any real-world detection already proven"*, rather
than that it would lose nothing.

Distinct from [name-the-check-you-could-not-run.md](name-the-check-you-could-not-run.md):
there no check ran at all — the binary is missing, the credential does not exist — and the
remedy is disclosure. Here the check ran, was capable of failing, and returned a genuine
negative; the defect is reporting it without the window that makes it true. Distinct from
[a-tool-claim-needs-a-reproduction-not-a-citation.md](a-tool-claim-needs-a-reproduction-not-a-citation.md),
whose remedy is to go and run it: this rule starts the moment you have. Distinct from the
vacuous-green family —
[a-cargo-run-that-compiled-nothing-verified-nothing.md](a-cargo-run-that-compiled-nothing-verified-nothing.md),
[verify-lint-fixes-on-the-toolchain-that-produces-them.md](verify-lint-fixes-on-the-toolchain-that-produces-them.md),
[../testing/a-mutation-only-counts-if-the-run-could-have-failed.md](../testing/a-mutation-only-counts-if-the-run-could-have-failed.md)
— in all of those the run *could not* have produced the failure, so the observation is
empty; here the observation is real and only the inference drawn from it overreaches.
Distinct from
[../comments-documentation/a-correct-conclusion-does-not-vouch-for-its-supporting-fact.md](../comments-documentation/a-correct-conclusion-does-not-vouch-for-its-supporting-fact.md):
a partial sample reported as a total — the same KYO-688 "ZERO runs ... across 64 cron runs"
claim, same review-log entry — is that rule's shape, not this one; reach for it when the
overreach is a real result read selectively (the column you liked, minus the tool's own
verdict on the rest), and for this rule when the run stayed within its own bound and was
still generalised past its duration or its correlation promoted to mechanism.
Bordering on
[../comments-documentation/name-the-invariant-not-a-count.md](../comments-documentation/name-the-invariant-not-a-count.md):
a 🟢 in `2026-09-11`'s *"KYO-712: bind review signature to the diff it approved (cycle 2)"*
flagged a comment claiming a grep *"finds exactly one hit across the whole tree"* that
returned 9 the instant its own diff landed. A tally offered as a safety argument is that
rule's; this one governs the conclusion drawn from a negative run, number or no number.

See also
[../comments-documentation/withdraw-a-claim-from-everything-that-carried-it.md](../comments-documentation/withdraw-a-claim-from-everything-that-carried-it.md)
— it mines nearly the same KYO-692/713 review-log cycles for an adjacent but distinct
failure: propagating a withdrawal to every tracked file that still carries the discredited
claim, once the claim is already known wrong, versus this rule's discipline of stating the
bound at authoring time so the claim is never overreached to begin with.
