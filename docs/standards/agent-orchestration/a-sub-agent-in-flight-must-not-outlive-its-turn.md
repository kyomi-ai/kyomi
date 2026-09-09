# Never end a turn with a sub-agent in flight that nothing left in the turn will consume

The durable rule is mode-independent, and it is worth stating before anything about
`claude -p` or `run_in_background`, because those details have already changed once (see
*What's changed since KYO-546*, below) and the rule needs to survive the next change too:
**if you dispatch a sub-agent whose result this turn needs, do not end the turn until you
have actually consumed that result.** "I'll check back when it returns" is not a plan, it
is a bet on whatever the harness happens to do with an orphaned background task at the
moment your process exits or your session goes idle — and that behavior is an
implementation detail, not a contract.

## `run_in_background` is mode-dependent, not a fixed switch

The Agent tool's own input schema is not the same in every session, and code copied from
one mode into the other silently does the wrong thing:

| Session type | `run_in_background` in the Agent tool schema | Effect of passing it |
|---|---|---|
| `claude -p` (headless/autonomous/cron) | Present (`description, isolation, model, prompt, run_in_background, subagent_type`) | Controls real scheduling — `false` blocks the tool call until the sub-agent finishes (measured, below) |
| Interactive session | Absent (`description, isolation, model, prompt, subagent_type`) | Silently ignored — there is no parameter for the harness to read |

This is why the previous version of this rule's RIGHT example — pass
`run_in_background: false` — cannot stand alone as "the fix": in an interactive session
that field doesn't exist, so writing it accomplishes nothing, and an agent that copied the
snippet there would believe it had forced foreground dispatch when it had done nothing at
all. The field is only ever a lever under `claude -p`. The invariant above is what still
holds in both places: don't end the turn before the result is in hand, whether or not a
parameter exists to help you enforce that.

## What's changed since KYO-546

KYO-546 (2026-08) found that under `claude -p`, a sub-agent left `started_in_background`
when the parent hit `end_turn` was killed with the process — `subagent_stats.killed.system`
— while the parent still reported `"is_error":false`, `"subtype":"success"`. That is why
this rule exists, and the six-attempt table below (unchanged since KYO-546) is the evidence
for it.

**That specific kill mechanism is now historical — it does not reproduce on the current
harness.** Reproduced 2026-09-09 on Claude Code 2.1.258 (installed 2026-09-02 11:20 local),
Fedora Linux 7.0.9-204.fc44.x86_64, per the reproduction discipline in
[a-tool-claim-needs-a-reproduction-not-a-citation.md](../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md):

1. Under `claude -p`, foreground dispatch genuinely blocks the tool call:
   `requested={background:0,foreground:1}`, `started_in_background=0`, `completed=1`,
   `killed.system=0`, `duration_ms=167150` for a ~150-second sub-agent task.
2. Under `claude -p`, a background sub-agent now **survives** the parent's `end_turn`. The
   parent ended its turn at 5.9s; the sub-agent process lived 166s, and the harness
   re-invoked the session and emitted a second `"type":"result"` reporting the sub-agent's
   completion. `requested={background:1,foreground:0}`, `started_in_background=1`,
   `completed=1`, `killed={parent:0,user:0,system:0}`.
3. `scripts/audit-agent-run-deaths.sh` over the post-upgrade window found **zero** runs with
   `killed.system > 0` across 64 cron runs. The one such run in the last 14 days —
   `2026-08-29 02:39:35Z session=fb3edb6b`, a KYO-468 attempt — predates the upgrade (it is
   attempt 6 in the table below). Both audit invocations still exited 3 /
   `COULD NOT COMPLETE` rather than a clean pass: a few older runs in the window are
   INDETERMINATE and the script fails closed on those rather than assuming they were fine,
   so "zero kills found" is not the same claim as "a clean exit."

In other words: the parameter that used to be silently fatal under `claude -p` (background,
unblocked, turn ends, kill) is now silently *survived* (background, unblocked, turn ends,
harness re-invokes anyway). That is a better outcome today, but it is still an
implementation detail of the current harness build, not a guarantee this rule can be built
on — which is exactly why the mode-independent invariant at the top, not either mechanism,
is the thing to internalize. KYO-688 tracked resolving whether the old mechanism still
applied; it is answered and shipped as PR #503, which also lands a tracked reproduction
script, `scripts/repro-headless-subagent-survival.sh`, so this can be re-checked
mechanically after any future harness upgrade instead of re-litigated from memory. That
script is not on this branch — it ships with KYO-688/#503.

## The KYO-468 history that motivated the rule

**KYO-468 (BigQuery "Discover Available" unreachable in create mode) died to the
now-historical kill mechanism, six times in a row**, always 10-16 minutes in, always before
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
`run_in_background` exists to help you express it. Under `claude -p`, the concrete way to
honor this today is foreground dispatch (`run_in_background: false`), because that is
measured to block the tool call until completion. Interactively, honor it by discipline:
don't tell the user "I'll check back" and end your turn — keep the turn open until you've
read and used the sub-agent's report. If a genuinely fire-and-forget background dispatch is
unavoidable, treat its result as something nothing in the current run depends on — not as
something a later turn will "pick up," since neither a guaranteed later turn (`claude -p`)
nor a documented delivery contract (either mode) can be relied on to make that true.

```
WRONG — orchestrator dispatches a sub-agent whose result this turn needs, then ends its
own turn to "wait" for it. This is wrong in both modes, for different reasons: under
`claude -p` there may be no later turn at all; interactively, ending the turn already
reports back to the user before the result the report depends on has arrived.

Agent({
    description: "Implement KYO-468",
    subagent_type: "feature-implementation-engineer",
    prompt: "...",
    run_in_background: true,   // or simply omitted under claude -p — same default;
                                // has no effect at all interactively, where the
                                // parameter doesn't exist in the schema
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
    run_in_background: false,   // claude -p only — forces the tool call to block;
                                 // this key does not exist interactively, so it is
                                 // not itself "the fix" there — see the table above
})
// Measured under claude -p: the tool call does not return until the sub-agent
// finishes, so there is nothing left "in flight" when this turn ends. Interactively,
// achieve the same outcome by not ending the turn — i.e. not sending a final message
// that implies completion — until this call has returned and its report is read.
```

KYO-546 (this standard, plus the detection tooling), KYO-468 (the ticket that died six times
before the cause was found), and KYO-688 (re-measured the mechanism against the current
harness, found it superseded, and shipped the reproduction script this file now cites as
landing with PR #503).
