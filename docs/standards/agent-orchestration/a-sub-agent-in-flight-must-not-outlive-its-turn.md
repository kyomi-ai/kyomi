# Never end a turn with a sub-agent in flight that nothing left in the turn will consume

The durable rule is mode-independent, and it is worth stating before anything about
`claude -p` or `run_in_background`, because those details are properties of a particular
harness build rather than contracts (see *What has and hasn't changed since KYO-546*,
below) and the rule needs to survive the next build too:
**if you dispatch a sub-agent whose result this turn needs, do not end the turn until you
have actually consumed that result.** "I'll check back when it returns" is not a plan, it
is a bet on whatever the harness happens to do with an orphaned background task at the
moment your process exits or your session goes idle — and that behavior is an
implementation detail, not a contract.

## Whether the Agent tool schema offers `run_in_background` is gated, not fixed

The Agent tool's own input schema is not the same in every session, and a snippet copied
into a session that doesn't offer the parameter silently does nothing — read out of build
2.1.267's own bundle rather than observed in a session, which is why no row below records
it: on the gated branch the schema omits the field (`e.omit({run_in_background:!0})`) and
strips unknown keys rather than rejecting them, so the value is dropped with no
`InputValidationError`. The harness does not attribute that variation to session type — it
attributes it to **either of two gates**. Its own field documentation for the requested-mode
counters reads, verbatim in build 2.1.267: *"Spawns by the run_in_background value the model
passed; all count as unset while the parameter is not offered (background tasks disabled, or
the fork gate on)."*

Three things have actually been observed on this box, each recorded here with the harness
build it was taken on. They stay listed as observations rather than being folded into a rule,
because availability is keyed on those two gates rather than on session type — these three
happen to sort by session type, but that is a correlation this box's configuration produces,
not the mechanism the harness documents, so a rule keyed on session type would be keyed on
the wrong thing:

| Date | Harness | Observed in | `run_in_background` in the Agent tool schema |
|---|---|---|---|
| 2026-09-09 | 2.1.258 | `claude -p` parent (`scripts/repro-headless-subagent-survival.sh schema`) | Present (`description, isolation, model, prompt, run_in_background, subagent_type`) |
| 2026-09-09 | 2.1.258 | Interactive session, same harness build | Absent (`description, isolation, model, prompt, subagent_type`) |
| 2026-09-11 | 2.1.267 | Sub-agent dispatched by a `claude -p` parent | Present |

Where the parameter *is* offered it controls real scheduling: on 2026-09-11, harness 2.1.267,
a `claude -p` parent passed `run_in_background: false`, the call was accepted with no
`InputValidationError`, and the Agent tool blocked for 418 seconds until the sub-agent
returned.

Two consequences. First, this is why the previous version of this rule's RIGHT example —
pass `run_in_background: false` — cannot stand alone as "the fix": where the field is not
offered, writing it accomplishes nothing, and an agent that copied the snippet there would
believe it had forced foreground dispatch when it had done nothing at all. Second, do not
infer the field's presence from the kind of session you think you are in; read the schema
you were actually handed. The invariant above is what holds either way: don't end the turn
before the result is in hand, whether or not a parameter exists to help you enforce that.

## What has and hasn't changed since KYO-546

KYO-546 (2026-08) found that under `claude -p`, a sub-agent left `started_in_background`
when the parent hit `end_turn` was killed with the process — `subagent_stats.killed.system`
— while the parent still reported `"is_error":false`, `"subtype":"success"`. That is why
this rule exists, and the six-attempt table below (unchanged since KYO-546) is the evidence
for it.

**That kill mechanism did not reproduce within one measured bound — which is a bound, not an
all-clear.** Re-tested 2026-09-09 on Claude Code 2.1.258 (installed 2026-09-02 11:20 local),
Fedora Linux 7.0.9-204.fc44.x86_64, per the reproduction discipline in
[a-tool-claim-needs-a-reproduction-not-a-citation.md](../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md):

1. Under `claude -p`, dispatching *without* a `run_in_background` parameter genuinely blocks
   the tool call: `requested={background:0,foreground:1}`, `started_in_background=0`,
   `completed=1`, `killed.system=0`, `duration_ms=167150` for a ~150-second sub-agent task.
2. Under `claude -p`, a background sub-agent outlived the parent's `end_turn`. The parent
   ended its turn at 5.9s; the sub-agent process lived 166s, and the harness re-invoked the
   session and emitted a second `"type":"result"` reporting the sub-agent's completion.
   `requested={background:1,foreground:0}`, `started_in_background=1`, `completed=1`,
   `killed={parent:0,user:0,system:0}`.
3. `scripts/audit-agent-run-deaths.sh` over the post-upgrade window found **zero** runs with
   `killed.system > 0` among the **62 of 64** cron runs it could assess; the other 2 were
   INDETERMINATE. The one such run in the last 14 days — `2026-08-29 02:39:35Z
   session=fb3edb6b`, a KYO-468 attempt — predates the upgrade (it is attempt 6 in the table
   below). Both audit invocations still exited 3 / `COULD NOT COMPLETE` rather than a clean
   pass: the script fails closed on those INDETERMINATE runs rather than assuming they were
   fine, so "zero kills found" is not the same claim as "a clean exit."

**Read that as a bound.** What it establishes is narrow: a background sub-agent of about 150
seconds, on harness 2.1.258, was not killed at its parent's `end_turn`. It does not establish
that the KYO-546 failure class is gone, and two facts sit directly against reading it that
way:

- **The harness still documents the mechanism.** Build 2.1.267's own field documentation for
  `subagent_stats.killed` reads, verbatim: *"system = by Claude Code itself (the
  --max-budget-usd halt, the sweep of background subagents when an SDK or IDE client
  interrupts, or -p giving up on a background subagent still running at its wait
  ceiling)"*. The identical text is present in 2.1.245 and in 2.1.258 — the very build
  measured above. A *wait ceiling* is current documented behaviour, not removed behaviour.
- **The measurement is shorter than the failures were.** KYO-468's six deaths all occurred
  10-16 minutes into their runs. A 166-second sample cannot probe a ceiling above 166
  seconds, so it does not reach the durations that actually failed.

So: a long-running background sub-agent is unmeasured here, not cleared — which is exactly
why the mode-independent invariant at the top, not either mechanism, is the thing to
internalize. KYO-688 tracked resolving whether the old mechanism still applied; it is
answered for that bound and shipped as PR #503, which also lands a tracked reproduction
script, `scripts/repro-headless-subagent-survival.sh`, so this can be re-checked mechanically
after any future harness upgrade instead of re-litigated from memory. Note which build the
numbers belong to: they were taken on 2.1.258, and this box has run 2.1.267 since 2026-09-10
10:37, so they describe the build they name rather than whichever build is running now.

## The KYO-468 history that motivated the rule

**KYO-468 (BigQuery "Discover Available" unreachable in create mode) died to that kill
mechanism, six times in a row**, always 10-16 minutes in, always before
pushing anything. Every attempt's own final words, reconstructed from the journal, describe
the orchestrator waiting on a background sub-agent it had no way to actually wait for —
attempt 6's last message was *"Waiting on the implementer now. When it returns I'll run the
4a-gate…"*, immediately followed by cron's normal `CMDEND`. Verified across every cron run
record on this box at the time: `killed.system > 0` never occurred on a run with
`started_in_background == 0` — not one counterexample. The converse did not hold, and that
was the trap: several runs (sessions `fefb49bb`, `249b739a`, `e5ec864a`) also started a
background sub-agent and survived, purely because that particular sub-agent happened to
finish before the parent's own turn ended. That race was not something an orchestrator
controlled, so "it worked last time" was never evidence it was safe. The long, successful
KYO-468 sibling runs that actually opened a PR (46, 53, 56 turns — sessions `69ad41f9`,
`83969caa`, `654e91a0`) sidestepped the race entirely: they dispatched every sub-agent in
the foreground and had zero system kills between them.

| Attempt | Session | started_in_background | completed | killed.system |
|---|---|---|---|---|
| 1 | 976062a9 | 2 | 1 | 1 |
| 2 | 1c70589d | 2 | 1 | 1 |
| 3 | 6cea5fa5 | 2 | 0 | 2 |
| 4 | 811af9fa | 2 | 1 | 1 |
| 5 | 514d8a54 | 1 | 0 | 1 |
| 6 | fb3edb6b | 2 | 2 | 1 |

This is the same shape as
[a value that degrades to empty on failure must not reach a consumer that reads it as a real
result](../error-handling/empty-on-failure-must-not-look-like-a-real-result.md): a lost
sub-agent's output didn't vanish loudly, it vanished into a report that still said
`success`. `dmesg`/`journalctl -k` showed no OOM kill, because there wasn't one, and cron
logged a normal `CMDEND`. The only artifact that distinguished a real success from this
failure was a field three levels deep in a JSON blob (`subagent_stats.killed.system`) that
nothing was reading until KYO-546 added `scripts/audit-agent-run-deaths.sh` to read it.

**Rule:** never dispatch a sub-agent whose result you need and then end your turn before
that result is consumed — regardless of session type, and regardless of whether
`run_in_background` exists to help you express it. Where the schema you were handed offers
`run_in_background`, the concrete way to honor this is foreground dispatch
(`run_in_background: false`), because that is measured to block the tool call until
completion. Where it doesn't, honor it by discipline: don't tell the user "I'll check back"
and end your turn — keep the turn open until you've read and used the sub-agent's report.
If a genuinely fire-and-forget background dispatch is unavoidable, treat its result as
something nothing in the current run depends on — not as something a later turn will "pick
up," since neither a guaranteed later turn (`claude -p`) nor a documented delivery contract
(either mode) can be relied on to make that true.

```
WRONG — orchestrator dispatches a sub-agent whose result this turn needs, then ends its
own turn to "wait" for it. This is wrong in both modes, for different reasons: under
`claude -p` there may be no later turn at all; interactively, ending the turn already
reports back to the user before the result the report depends on has arrived.

Agent({
    description: "Implement KYO-468",
    subagent_type: "feature-implementation-engineer",
    prompt: "...",
    run_in_background: true,   // where the schema does not offer this parameter, the
                                // line has no effect at all — see the observations above
})
// "I'll report back when the implementer returns." <- there is no turn-scoped "when"
// in either mode: under claude -p the process may simply exit; interactively the
// current turn has already ended without the result it promised.

RIGHT — don't end the turn until the result is actually in hand. Under `claude -p`,
foreground dispatch makes this the tool call's own behavior:

Agent({
    description: "Implement KYO-468",
    subagent_type: "feature-implementation-engineer",
    prompt: "...",
    run_in_background: false,   // only where the schema offers it — measured to force
                                 // the tool call to block; where it is not offered this
                                 // key is not itself "the fix" — see the observations
})
// Measured under claude -p on 2.1.267 with an explicit false: the tool call did not
// return for 418 seconds, until the sub-agent had finished. On 2.1.258 the same
// blocking was measured with the parameter omitted rather than set to false, so that
// build's number is evidence about omission, not about this line. Either way nothing
// is left "in flight" when the turn ends. Where the parameter is not offered, achieve
// the same outcome by not ending the turn — i.e. not sending a final message that
// implies completion — until this call has returned and its report is read.
```

KYO-546 (this standard, plus the detection tooling), KYO-468 (the ticket that died six times
before the cause was found), and KYO-688 (re-measured the mechanism on harness 2.1.258,
established the bound recorded above, and shipped the reproduction script this file now
cites as landing with PR #503).
