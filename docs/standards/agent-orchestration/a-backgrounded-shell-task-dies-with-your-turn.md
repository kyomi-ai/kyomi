# A backgrounded Bash task dies with your turn — an Agent sub-agent does not

Two tools in this harness accept the same parameter name, `run_in_background`, and do
opposite things with it under `claude -p`. An `Agent` sub-agent left running in the
background survives the parent's `end_turn` — KYO-688 measured the harness re-invoking the
session to deliver its result (see
[a-sub-agent-in-flight-must-not-outlive-its-turn.md](a-sub-agent-in-flight-must-not-outlive-its-turn.md)).
A **Bash** tool task started with `run_in_background: true` does not: it is killed the
moment the parent's turn ends, silently, with no error anywhere in the chain. Reasoning
from one tool's lifetime to the other's is how a whole class of work gets destroyed, and
KYO-769 is the reproduction of exactly that.

## The incident

`/backlog needs-build` (`~/.local/bin/kyomi-needs-build-cron.sh`, a daily `claude -p` cron)
died three times in four days on ticket KYO-727, executed **zero** of its twelve checklist
items across those attempts, and left three tombstoned worktrees. Two of the three deaths
share the identical signature below; the third had a different cause and is not part of
this rule's evidence.

Both surviving logs are on disk and were re-read for this rule, not summarized from memory:

- `/tmp/kyomi-needs-build-20260912-033501.log` — session `20b18f10-7c57-4842-9398-eb6ffc862dd6`,
  build task `be8i92zsf`, summary `"Build the dev-server binary"`.
- `/tmp/kyomi-needs-build-20260914-033500.log` — session `1c895cb8-3cd0-4e9d-a121-fd9d35f3c564`,
  build task `bj58jzy7u`, summary `"Build server binary and WASM bundle"`.

In the `914` run, the orchestrator's own final message ends, verbatim:

> Waiting on the build to finish, then starting the server and running the twelve items in
> waves (item 7 alone, since its evidence is health-endpoint latency and needs a quiet box).

Immediately after that `"type":"result"` entry, the same log carries:

```
{"type":"system","subtype":"background_tasks_changed","tasks":[]}
{"type":"system","subtype":"task_updated","task_id":"bj58jzy7u","patch":{"status":"killed","end_time":1789321857108}}
{"type":"system","subtype":"task_notification","task_id":"bj58jzy7u","status":"stopped","summary":"Build server binary and WASM bundle"}
```

(fields elided for width; `task_id`, `"status":"killed"`, and `"status":"stopped"` are each
byte-for-byte from the source line, not reconstructed). The `912` run's final message opens,
verbatim:

> Build is still compiling `kyomi-ui` (native). I'll let it finish — the harness will bring
> me back when it exits.

and is followed by the same three-line `background_tasks_changed` / `task_updated:killed` /
`task_notification:stopped` sequence against task `be8i92zsf`.

**That sentence is the entire defect, stated by the agent in its own words.** The harness
*does* bring you back for a backgrounded `Agent` sub-agent — KYO-688 measured exactly that.
It does **not** for a backgrounded Bash task. The `912` orchestrator held a true belief
about one tool and applied it to the other, which is precisely the conflation KYO-687 warns
against, and it is the closest thing to a confession this class of failure is ever going to
produce. Neither run consumed the task's result before generation stopped; the wording
differed, the kill did not.

## Why this needs a rule, not a footnote

**It reports success.** Both runs' own final `"type":"result"` object says
`"subtype":"success"`, `"is_error":false`, `"terminal_reason":"completed"` — `914` at
`"duration_ms":949324` (37 and 41 `num_turns` respectively for `912`/`914`). Cron logged a
normal exit both times. Nothing anywhere flags that the build the whole run existed to
produce was never observed to finish. That is what let the identical defect recur: attempt
1 (`912`) taught nothing to attempt 2 (`914`), because attempt 1 never looked like a
failure.

**"Never background it" is wrong in the other direction.** Backgrounding the build is not
optional — it is required. The Bash tool's own `timeout` parameter is capped "up to
600000ms (10 minutes)", raisable only via `BASH_MAX_TIMEOUT_MS`, which is unset in this
environment (confirmed: `env | grep -i BASH_MAX_TIMEOUT_MS` on this box, this session,
returns nothing), while a cold build here runs up to ~1 hour per
`.claude/build-test.md`. A single foreground Bash call cannot span that. The defect is not
that the task was backgrounded — it is ending the turn while it was still outstanding.

## The remedy is an invariant, not a tool name

The instinct is to write "call `TaskOutput` to block until the task completes" as the fix.
Resist writing that sentence the way it just appeared: **`TaskOutput` is real, and both
dying runs already had it.** Both cron sessions' own `"type":"system","subtype":"init"`
event lists their granted tools verbatim, and the exact substring `"TaskOutput","TaskStop"`
appears in that list in *both* `912` and `914`. A full pass over every `tool_use` block in
both logs (Python, matching `type == "tool_use" and name == "TaskOutput"`) found **zero**
invocations in either. The tool was sitting in the grant the entire time; the orchestrator
never called it, choosing instead to end its turn on a sentence that assumed a later turn
would pick the result up — the exact anti-pattern
[a-sub-agent-in-flight-must-not-outlive-its-turn.md](a-sub-agent-in-flight-must-not-outlive-its-turn.md)
already names for the `Agent` tool. This was never a missing-capability problem.

Two things follow, and the second is the one to actually act on:

1. If your own session's tool grant offers a blocking or result-retrieval call for a
   background task, use it — but confirm the name and its parameters against the tool
   listing you were actually handed before calling it. `TaskOutput` does not appear in
   *this rule's own author's* granted tools (checked via `ToolSearch`, which returned no
   match), so this file does not assert a parameter shape for it — doing so would be
   exactly the failure
   [an-instruction-must-name-a-mechanism-that-exists.md](an-instruction-must-name-a-mechanism-that-exists.md)
   describes: a plausible-sounding argument list nobody has run. Availability here is
   gated per session, the same way the sibling file's `run_in_background` table shows for
   the `Agent` tool — read the schema you were actually given, not one quoted from
   somewhere else.
2. **Regardless of which specific call your grant offers, the checkable property is:** do
   not produce a message that reads as a stopping point — anything resembling "I'll check
   back," "waiting on the build," or simply falling silent about the outstanding task —
   while a backgrounded Bash task this run still needs is unresolved. Keep issuing tool
   calls that work toward or wait on that task until you have actually consumed a terminal
   status for it, in the same turn.

A `Monitor` does not substitute for this. Its own tool description says so directly: "you
keep working and notifications arrive in the chat" — that is a live, interactive-session
model where a later turn is guaranteed. Under `claude -p` there may be no later turn at
all; the notification has nowhere to land once the process exits at `end_turn`.

```
WRONG — matches both KYO-727 deaths. The task is correctly backgrounded (it must be, at
~1hr against a 10-minute foreground cap), but the turn ends before its result is consumed.

Bash({ command: "cargo build --locked --profile dev-server", run_in_background: true })
// "Waiting on the build to finish, then starting the server and running the twelve
// items in waves." <- this is the entire remainder of the turn. Under claude -p the
// process now exits; the background task is killed with it, unread.

RIGHT — the task stays backgrounded (required), but the turn does not end while it is
outstanding. Concretely: keep calling whatever your session's grant actually offers for
this — confirmed against the tool listing, not assumed — until it reports a terminal
status, and only then write a message that describes what happened.

Bash({ command: "cargo build --locked --profile dev-server", run_in_background: true })
// loop: call the retrieval/blocking tool your grant provides (name and parameters
// verified against your own tool listing) until it reports the task finished; only
// then move on to starting the server and running the checklist.
```

## Division of labor with the neighboring rule

[a-sub-agent-in-flight-must-not-outlive-its-turn.md](a-sub-agent-in-flight-must-not-outlive-its-turn.md)
states the invariant for the `Agent` tool and is careful to note that tool's
`run_in_background` schema is itself gated — present in some sessions, absent in others —
and that where present, foreground dispatch (`run_in_background: false`) was *measured* to
block the call under `claude -p`. It does not mention Bash background tasks anywhere, and
should not be read as covering them: KYO-688 measured an `Agent` sub-agent surviving
`end_turn`; this file's own evidence, one line above, is a Bash task being killed by it,
same day, same class of cron session. Same invariant — never end a turn with unread work
outstanding — opposite tool, opposite measured lifetime. Neither file supersedes the
other; a reader who needs the Bash-side behavior should land here, not infer it from the
`Agent`-side file.

Environment for every tool claim in this file: Claude Code **2.1.270**, session-local
`ToolSearch` returning no match for `TaskOutput`; the two cron logs quoted above also
record their own build in their `init` events, and the two differ: the `912` run ran on
`"claude_code_version":"2.1.267"`, the `914` run on `"2.1.270"`. The defect reproduced on
both, which is the point -- it is not an artefact of a single build. Fedora Linux, observed
**2026-09-14**, per
[a-tool-claim-needs-a-reproduction-not-a-citation.md](../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md).

KYO-769 (this diagnosis), KYO-727 (the ticket that died three times, two of them to this
defect), KYO-688 (established the `Agent`-side survival this file's title contrasts
against), KYO-687 (first warned against conflating the two tools' `run_in_background`),
KYO-546 (the original sub-agent-kill investigation this rule's sibling was built from),
KYO-691 (open: the `build-test.md` / Step 5c wording that told the orchestrator to
background-and-wait without saying how to actually wait).
