# Under a headless (`claude -p`) run, never dispatch a sub-agent in the background and then end your turn

An interactive session has a human to type the next message, so a background sub-agent left
running when a turn ends simply resumes reporting whenever it finishes — the session is still
alive to receive it. A `claude -p` run has no such thing. There is no message queue behind it,
no human, and no next turn: the process exits at `end_turn`, and every sub-agent still marked
`started_in_background` at that instant is killed with it. The harness records this as
`subagent_stats.killed.system`, and it is not a crash — the *parent* run genuinely did finish
cleanly, so it reports exactly what it saw: `"is_error":false`, `"stop_reason":"end_turn"`,
`"terminal_reason":"completed"`, `"subtype":"success"`. The work the killed sub-agent was doing
is simply gone, and nothing in that report says so.

This is what makes the failure dangerous rather than merely wasteful: it is silent by
construction. `dmesg`/`journalctl -k` show no OOM kill, because there wasn't one. Cron logs a
normal `CMDEND`. The only artifact that distinguishes a real success from this failure is a
field three levels deep in a JSON blob (`subagent_stats.killed.system`) that nothing was reading
until KYO-546 added `scripts/audit-agent-run-deaths.sh` to read it. This is the same shape as
[a value that degrades to empty on failure must not reach a consumer that reads it as a real
result](../error-handling/empty-on-failure-must-not-look-like-a-real-result.md): a lost
sub-agent's output doesn't vanish loudly, it vanishes into a report that still says `success`.

**KYO-468 (BigQuery "Discover Available" unreachable in create mode) died to exactly this, six
times in a row**, always 10-16 minutes in, always before pushing anything. Every attempt's own
final words, reconstructed from the journal, describe the orchestrator waiting on a background
sub-agent it had no way to actually wait for — attempt 6's last message was *"Waiting on the
implementer now. When it returns I'll run the 4a-gate…"*, immediately followed by cron's normal
`CMDEND`. Verified across every cron run record on this box: `killed.system > 0` never occurred
on a run with `started_in_background == 0` — not one counterexample. The converse does not
hold, and that is the trap: several runs (sessions `fefb49bb`, `249b739a`, `e5ec864a`) also
started a background sub-agent and survived, purely because that particular sub-agent happened
to finish before the parent's own turn ended. That race is not something an orchestrator
controls, so "it worked last time" is not evidence it is safe. The long, successful KYO-468
sibling runs that actually opened a PR (46, 53, 56 turns — sessions `69ad41f9`, `83969caa`,
`654e91a0`) sidestep the race entirely: they dispatch every sub-agent in the foreground and had
zero system kills between them.

| Attempt | Session | started_in_background | completed | killed.system |
|---|---|---|---|---|
| 1 | 976062a9 | 2 | 1 | 1 |
| 2 | 1c70589d | 2 | 1 | 1 |
| 3 | 6cea5fa5 | 2 | 0 | 2 |
| 4 | 811af9fa | 2 | 1 | 1 |
| 5 | 514d8a54 | 1 | 0 | 1 |
| 6 | fb3edb6b | 2 | 2 | 1 |

**Rule:** under `claude -p` — any autonomous or cron-driven run, including every `/backlog`,
`/backlog-fast`, `/kyomi-backlog`, and `/loop` invocation — never call the Agent tool with
`run_in_background: true` (or omit the parameter, since it defaults to background) and then end
your own turn to "wait" for the result. Either dispatch the sub-agent in the foreground
(`run_in_background: false`) and block on it directly, or restructure the work so nothing
depends on a background sub-agent surviving past the current turn. If a genuinely
fire-and-forget background dispatch is unavoidable, the turn that dispatched it must not be the
run's last turn — but under a single-shot `claude -p` invocation there is no reliable way to
guarantee a later turn will run at all, so in practice this reduces to: under `claude -p`,
foreground only.

```
WRONG — orchestrator ends its turn "waiting" for a background sub-agent under `claude -p`.
The process exits at end_turn, the sub-agent is killed mid-work, and the run still reports
subtype:"success":

Agent({
    description: "Implement KYO-468",
    subagent_type: "feature-implementation-engineer",
    prompt: "...",
    run_in_background: true,   // or simply omitted — same default
})
// "I'll report back when the implementer returns." <- there is no "when" under claude -p;
// the run ends here, and the harness kills the implementer with it.

RIGHT — dispatch in the foreground and actually block on the result before the turn ends:

Agent({
    description: "Implement KYO-468",
    subagent_type: "feature-implementation-engineer",
    prompt: "...",
    run_in_background: false,
})
// The tool call itself doesn't return until the sub-agent finishes, so there is
// nothing left "in flight" when this turn — or the process — ends.
```

KYO-546 (this standard, plus the detection tooling) and KYO-468 (the ticket that died six times
before the cause was found).
