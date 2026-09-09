# An operating instruction must name a mechanism that exists, or state an invariant instead

Operating docs tell an agent to *do* something: pass this argument, arm that watcher, hold this
lock. When the named mechanism does not exist, nothing reports it. The tool call is accepted and
the argument ignored, or the instruction is "followed" by writing a sentence that claims it was.
The agent then holds a **false belief that it complied**, and reasons from that belief — which is
worse than not having the rule at all, because an absent rule leaves the agent uncertain while a
phantom one leaves it confident.

This fails silently in a way a wrong *fact* does not. A doc that misstates a line number gets
caught the first time someone follows it. A doc that names a non-existent mechanism produces no
error at any point: not at authoring, not at review, not at execution.

The remedy in both recorded cases was the same. An **invariant** — a property the reader can
check by looking at what they actually did — needs no mechanism to be real, and stays true when
the tooling changes underneath it. "Never end a turn with a sub-agent in flight" is checkable by
inspection; "pass `run_in_background: false`" was checkable only against a parameter that was
never there.

**Rule:** Before writing "do X by means of Y" into an operating doc, confirm Y exists — read the
tool's schema, or `grep` the corpus for the primitive you are about to name. If you cannot point
at it, do not name it: state the observable property the reader must end up holding, and let them
choose the means. When you inherit an instruction of this shape, do not merely correct the
argument — restate the rule as the invariant it was standing in for, or the next tooling change
reintroduces it.

The pair below is **illustrative, not a quotation**: the WRONG side composites two
near-verbatim lines from two different sources — the rule as it stood in
`kyomi-private/skills/build-test.md`, and the cycle-1 draft that was deleted before merge.

```markdown
WRONG — names an argument the tool does not accept, and a primitive that does not exist:

  Pass `run_in_background: false` on every `Agent` call.

  After dispatching, arm a fallback so a lost notification cannot strand the run silently.

RIGHT — states the property the reader must hold, checkable without any named mechanism:

  Never end a turn with a sub-agent in flight.

  After dispatching, keep doing work that does not depend on the result. Writing a sentence
  about waiting is not a mechanism.
```

Mined from two instances, both in the KYO-687 review (`docs/review-logs/2026-09-09.md`, the
*"restate the Agent-tool `run_in_background` rule as an invariant"* entries):

- **The rule being corrected.** KYO-546 established *"pass `run_in_background: false` on every
  `Agent` call"* across three canonical operating docs. The `Agent` tool has no such property
  (`description`, `prompt`, `subagent_type`, `model`, `isolation`, under
  `additionalProperties: false`), and a probe passing it was accepted with no
  `InputValidationError` and launched asynchronously anyway. It survived in three docs for weeks
  precisely because nothing ever rejected it.
- **The fix's own first draft**, cycle 1, which instructed the reader to *"arm a fallback so a
  lost notification cannot strand the run silently"*. `grep` found that phrase exactly once in
  the corpus — its own occurrence — and no fallback primitive is defined anywhere. The same
  defect, reintroduced inside the document correcting it, and caught only by review.

Nearest siblings, and how this differs.
[a-sub-agent-in-flight-must-not-outlive-its-turn.md](a-sub-agent-in-flight-must-not-outlive-its-turn.md)
is the specific rule that instantiated this failure; its defective form was corrected in the
same commit that renamed it (KYO-692). This file is the general shape, not that incident.
[../comments-documentation/no-guarantee-stronger-than-code-enforces.md](../comments-documentation/no-guarantee-stronger-than-code-enforces.md)
is about a *comment* claiming more than real code provides — there the mechanism exists and the
claim overreaches; here the named thing is absent entirely.
[../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md](../build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md)
is the closest: it requires reproducing a claim about tool behaviour before resting on it.
That rule governs whether a claim is *verified*; this one governs the **shape of the
instruction** — an instruction whose referent does not exist cannot be verified even in
principle, and the fix is not a better citation but a different kind of sentence. Finally,
[../comments-documentation/verify-every-identifier-in-a-doc-code-example.md](../comments-documentation/verify-every-identifier-in-a-doc-code-example.md)
overlaps on one axis — both catch a name that resolves to nothing — but it is discharged by
grepping each *identifier* against the source, and it reaches prose citations too, not only
code blocks. Neither of this file's instances is reachable that way. The first names an
identifier that **does** resolve — `run_in_background` is a real parameter on the `Bash`
tool, just not on the one being called, so no grep for the name would flag it. The second
names **no identifier at all**: "arm a fallback" is a capability, and there is nothing to
look up. That is the axis — that rule asks whether a name resolves; this one asks whether
the *mechanism an instruction directs the reader to use* exists.
