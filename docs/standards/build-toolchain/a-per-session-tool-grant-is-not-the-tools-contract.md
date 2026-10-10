# A per-session tool grant is a sample, not the tool's contract

The sibling rule for external tools says: don't cite the documentation, go and run it. That
works because `curl` behaves the same for everyone. The harness an agent runs inside does
not. Which tools a session is handed, and which parameters those tools expose, are **gated
per session** — by configuration, by a feature gate, by whether the caller is a teammate or
a `claude -p` parent. Running it is still the right first move, but the result is an
observation about *your* grant, and writing it into `skills/`, `.claude/` or a standards
file turns it into a universal claim about a surface that was never universal.

The cost is not theoretical. These docs exist to tell a future agent which parameter to
pass; an agent that copies a parameter its own session was not granted believes it has
forced a behaviour it has not, and the run reports success either way. That is the same
false-green this family of docs was written to eliminate, reintroduced by the document
itself.

Two shapes recur:

- **Two reproductions, opposite answers, both asserted.** One open PR reproduced
  `run_in_background` as *absent* from the Agent tool's schema and silently ignored when
  passed; the reviewer of a second PR editing the same file reproduced it as *present* in
  their own session. A third observation, recorded in
  [../agent-orchestration/a-sub-agent-in-flight-must-not-outlive-its-turn.md](../agent-orchestration/a-sub-agent-in-flight-must-not-outlive-its-turn.md),
  has a `claude -p` parent passing `run_in_background: false` on 2026-09-11 and the call
  blocking for 418 seconds. Every one of those is a true report, none said "in my session",
  and the doc ended up carrying three mutually inconsistent stances on one fact inside forty
  lines.
- **The basis is not named, so nobody can tell what kind of claim it is.** *"Checked on
  build 2.1.267, where the schema omits the field"* does not say whether that came from a
  session or from reading the installed binary — and the two answer different questions. A
  claim sourced to a session is gated by construction; a claim sourced to the binary is not.
  A reader who cannot tell cannot re-check it, and cannot know whether their own session
  contradicting it is a bug or the expected variation.

**Rule:** Before asserting how a tool an agent will be handed behaves, decide which of two
claims you are making and write it as that claim. If the basis is your own session, say so
and date it — *"this session's reproduction, not a guarantee about yours"* — and write the
instruction as check-then-branch: confirm the tool or parameter is in your own grant
(`ToolSearch` for `select:<Tool>`), and state plainly what to do when it is not, including
the case where nothing in your grant can do the job. If the claim has to hold for every
reader, settle it against the artefact that defines the surface — the installed binary at a
named version, not any session — and name that as the basis. And when two reproductions
disagree, the disagreement *is* the finding: record both observations and assert neither
until something outside a session settles it.

**WRONG** — `git show b41efcb:skills/build-test.md` in `~/repos/kyomi-private`, lines
249-256, elided at `...` and cut at the full stop partway through line 256; the paragraph
is pre-existing and still stands on `main`. A flat universal about a gated surface,
followed by a concrete instruction that does nothing at all in a session where the
parameter is not offered — and the reader has no way to find that out:

```markdown
**The Agent tool backgrounds by default.** Under `claude -p` — every cron run — there is no
turn after your last one, so a "you'll be notified when it completes" notification has
nowhere to arrive. The process exits at `end_turn` and the in-flight sub-agent is killed.
...
Pass **`run_in_background: false`** on every `Agent` call.
```

**RIGHT** — the same file at `e35eed0` on `origin/jason/kyo-691-build-background-reconcile`
(**in flight in `kyomi-private`, not merged as of 2026-09-15**), lines 249-267, elided at
`...`. Same underlying requirement, but the grant is checked first, the parameter values
are scoped to the session that observed them and dated, and the absent case has somewhere
to go that is not a false green:

```markdown
**`TaskOutput` is gated per session — confirm you were actually handed it before you
plan around it.** Check your own tool listing (`ToolSearch` for `select:TaskOutput`);
sessions differ, and a session that was not granted it will not acquire it by following
this paragraph. Where it *is* granted, its parameters were read from its own tool
description on 2026-09-14: `block` defaults to `true`, and `timeout` defaults to 30000ms
with a **maximum of 600000ms** ... Treat all of that as this session's reproduction, not a
guarantee about yours.

If `TaskOutput` is absent from your grant, or a future harness drops it, the requirement
does not relax — find whatever tool your session exposes with the same blocking
semantics and use that. **Do not fall back to background-and-hope** ... If nothing in
your grant can block, say so and report the build as unverified; that is a useful result,
and a false green is not.
```

Real precedent — three tickets, two log days, and the flagship cost three review cycles:

- **KYO-691**, review log `2026-09-14`, heading *"KYO-691: reconcile build-backgrounding
  contradiction (build-test.md / backlog-fast/SKILL.md)"* — two 🟡 on one diff. The first:
  the diff asserted `TaskOutput`'s `block`/`timeout` shape as fact *"read from its own tool
  description"*, when the standard it cited had **declined** to assert that shape because
  `ToolSearch` returned no match in its author's session — *"a plausible-sounding argument
  list nobody has run"* — and *"This reviewer's own session also returns no `TaskOutput`
  match via `ToolSearch`."* The second: "This reviewer's own Agent tool schema *does* show a
  `run_in_background` property, contradicting PR #38's claim, so the fact is unsettled in
  either direction and neither PR should assert it without a fresh reproduction."
- **Same ticket, the *cycle 2* entry** — 🟡, and the shape is why this rule is about
  authoring rather than about one paragraph. Finding 1 was resolved exactly as the RIGHT
  block above: the text *"now leads with a per-session `ToolSearch` check, frames the
  `block`/`timeout` values as one session's reproduction rather than a guarantee, and adds
  an explicit fallback"*. Finding 2 was not — the unhedged version reappeared in *other* new
  text in the same diff, and the pre-existing WRONG paragraph above stood four paragraphs
  away, leaving *"three passages within ~40 lines of the same section"* taking three
  different stances on the same fact. Cycle 3 closed it by naming PR #38's actual finding
  and hedging the instruction to best-effort.
- **KYO-769**, `2026-09-14`, heading *"KYO-769 Step 5c: fix orchestrator stalling on
  backgrounded builds under `claude -p` (re-review after nits)"* — 🟢, the unnamed-basis
  shape: the `TaskOutput` claims were *"exactly correct (verified twice, independently,
  against the installed binary) but the doc itself carries no citation of how/when this was
  verified"*. That reviewer had extracted the schema from
  `strings -a -n 8 ~/.local/share/claude/versions/2.1.270` and read the tool's `call()`
  implementation — the level of evidence that settles a claim for every reader — and the doc
  did not say so. The other 🟢 in the same entry is the environment-gated twin: the Bash
  tool's 600000ms ceiling was *"stated as an unconditional tool fact"* when it is
  `BASH_MAX_TIMEOUT_MS`-overridable. The cycle-1 entry for the same ticket (heading
  *"KYO-769 Step 5c: fix orchestrator stalling on backgrounded builds under `claude -p`"*)
  proposes this rule outright in its Notes: that level of verification *"seems warranted
  whenever a diff makes a specific, falsifiable claim about a tool's parameters or limits"*,
  and it is *"worth considering as a standard step for review of `.claude/`/`skills/` docs
  specifically."*
- **KYO-692 / KYO-713**, `2026-09-11`, headings *"KYO-692 (+KYO-713) rename and correct the
  headless sub-agent standard (PR #505)"* cycles 3 and 4 — 🟢 twice, both filed as
  *unattributed-mechanism-claim*, against the standard that now lives at
  [../agent-orchestration/a-sub-agent-in-flight-must-not-outlive-its-turn.md](../agent-orchestration/a-sub-agent-in-flight-must-not-outlive-its-turn.md).
  Cycle 3: the file's claim that a passed-but-ungranted parameter *"silently does nothing"*
  was *"the file's only harness-behaviour claim with no build + measurement attached, in a
  file that dates every other one"* — the reviewer verified it true against 2.1.267's own
  bundle (`jl()||G5() ? e.omit({run_in_background:!0}) : e`, and zod strips unknown keys
  rather than rejecting them) and asked only that the basis be recorded. Cycle 4: "`checked
  on build 2.1.267, where the schema omits the field` does not say *how* it was checked, and
  the observation table has no 2.1.267 absent-observation — the basis was a bundle read, not
  a session." (Emphasis is the log's own in both of these quotes and in KYO-691's above.)
  Both were evidence hygiene rather than correctness, and both cost a cycle.

Distinct from
[a-tool-claim-needs-a-reproduction-not-a-citation.md](a-tool-claim-needs-a-reproduction-not-a-citation.md):
that rule's subject is a tool whose behaviour is the same wherever you run it, and its
remedy — reproduce it in a throwaway directory — is where this rule *starts*. Here the
reproduction happened, was competently done, and still does not establish the claim, because
the thing reproduced was a grant rather than a contract. Reach for that rule when nobody ran
it; reach for this one when two people did and got different answers.

Distinct from
[a-negative-result-is-bounded-by-the-run-that-produced-it.md](a-negative-result-is-bounded-by-the-run-that-produced-it.md):
that rule bounds a *negative* by what the run could reach — its duration, its sample, its
build — and warns against promoting the sample's correlate into the mechanism. The claims
here are positive assertions about a surface, and the missing bound is *whose session*, not
how long it ran. The two meet where that rule's second shape lives (three schema
observations keyed on session type when the harness keys on gates); this rule is the
authoring-time discipline that keeps such a claim out of a doc in the first place.

Distinct from
[../agent-orchestration/a-sub-agent-in-flight-must-not-outlive-its-turn.md](../agent-orchestration/a-sub-agent-in-flight-must-not-outlive-its-turn.md):
that document settles one specific question — the Agent tool's `run_in_background` is gated,
here are the dated observations, read the schema you were handed — and is the worked example
of this rule done right, down to a per-observation build column. It is a domain fact about
one parameter. This rule is the general obligation the next such paragraph inherits, for a
tool nobody has written a table for yet.

See also
[../comments-documentation/no-guarantee-stronger-than-code-enforces.md](../comments-documentation/no-guarantee-stronger-than-code-enforces.md)
and
[../agent-orchestration/an-instruction-must-name-a-mechanism-that-exists.md](../agent-orchestration/an-instruction-must-name-a-mechanism-that-exists.md):
an instruction to pass a parameter your reader's session does not offer is the same defect
as an instruction naming a mechanism that does not exist — it is followed, it accomplishes
nothing, and the follower believes otherwise. The check-then-branch form above is what keeps
the instruction honest when the mechanism exists for some readers and not others.
