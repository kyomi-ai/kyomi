# A variant chosen for a call site is a claim about that call site — check it there, not against the value it replaces

When an overloaded field is split into a named enum, every existing construction
site has to be handed a variant. The cheapest way to pick one is *translation*:
this site used to pass `None`, and `AdapterPersists(None)` is the variant that
reproduces exactly what `None` used to do, so that is what it gets. The workspace
compiles, the sweep is complete, and a reviewer asking "did every construction
site get converted?" gets a clean answer at every single one of them.

That is not the question the split created. The new variant does not describe
what the field used to hold; it *asserts something about the caller* — this
surface already wrote the row itself, that one is relying on the callee to write
it. Translating the old value preserves the old behaviour, which is the one thing
the split exists to re-examine: the old field was a single overloaded slot, so at
some sites it was already the wrong answer and nothing had a name for that yet. A
mechanical translation makes those sites' wrongness official and gives it a
plausible-looking label, in a diff whose review notes read "all four call sites
converted correctly."

The failure is silent by construction. Every variant is legal at every call site
— that is what makes the enum useful — so there is no compile error, no lint, and
no sweep to redo. It surfaces later, one surface at a time, as a separate bug
report against whichever caller somebody happened to look at next.

Transplanting a *sibling's* fix is the same mistake wearing the other hat: once
one site's variant is corrected, the remaining wrong ones are not guaranteed to
want the same correction. Each still needs its own trace.

**Rule:** When you split an overloaded field into variants, do not derive each
call site's variant from the value it used to pass. For every site, read that
caller's own path — what it writes before the call, what the callee will write
after — and state, in a comment at the site, the fact that makes its variant
true. A site whose fact you cannot state is a site you have not checked. Pin each
one with a test that exercises that surface end to end and asserts the observable
consequence (one row, not two), not that the variant was passed; a test that
reads back the enum will agree with any translation. When a later ticket corrects
one site, re-derive the others rather than copying the correction across — see
[../code-organization/a-copied-pattern-must-carry-its-precondition.md](../code-organization/a-copied-pattern-must-carry-its-precondition.md).

```rust
// WRONG — the variant is the old `None` in a new spelling. `copilot.rs` and
// `execute_watch_inner` both pre-write the user row themselves, so neither is
// an "adapter persists it" caller; but `None` used to mean "no caller-chosen
// id", and this is the variant that reproduces that, so this is what they got.
user_message_persistence: kyomi_agent::UserMessagePersistence::AdapterPersists(None),
// `caller_persisted_id()` returns `None`, so `drop_pre_persisted_message`
// no-ops and `should_persist_new_message` never skips: the pre-written row
// stays in loaded context *and* `persist_after_chat` writes a second copy.

// RIGHT — the variant states what this caller actually did, and the id it
// names is the PK `prepare_copilot_message` already wrote.
user_message_persistence: kyomi_agent::UserMessagePersistence::CallerPersisted(
    prep.user_message_id,
),
```

Real precedent — one split, four surfaces, six days, three follow-up tickets:

- **KYO-492** (review log `2026-08-24`, `04:55`, Clean/approved) created
  `UserMessagePersistence` (`crates/kyomi-agent/src/adapter.rs`) precisely
  because the old `AgentExecutionConfig` field answered two questions at once
  (see [split-a-value-that-answers-two-questions.md](split-a-value-that-answers-two-questions.md)).
  The review verified the sweep exhaustively and recorded: *"All four call sites
  (`chat.rs` → `CallerPersisted`, `copilot.rs` → `AdapterPersists(None)`,
  `watch_execution.rs` ×2 → `AdapterPersists(None)`, `routes.rs` →
  `AdapterPersists(Some(id))`) converted correctly."* Every site was found;
  each site's variant was checked against the value it replaced, and against
  Slack's flow specifically — but not against copilot's or watch's own writes.
- **KYO-506** (`2026-08-29`, `21:10`) is where the cost showed up, as a
  cross-check made while reviewing an unrelated ticket:
  *"`copilot_service::prepare_copilot_message` writes the user row up front,
  then `copilot.rs` sets `UserMessagePersistence::AdapterPersists(None)`
  instead of `CallerPersisted`, so `drop_pre_persisted_message` never filters
  it out, and `CustomAgent::chat()` pushes a second copy that
  `persist_after_chat` then persists."*
- **KYO-554** (`2026-08-31`, `12:00`, Clean/approved) fixed copilot. The
  reviewer's mutation — reverting the test's `persistence` local to
  `AdapterPersists(None)` — reproduced the pre-fix state exactly: *"2 rows:
  `"what was Q4 revenue"` and `"[source: web, user_local_time: ...] what was
  Q4 revenue"`."* Two user rows for one turn, in production, for six days.
- **KYO-572** (`2026-08-31`, `08:57`, Clean/approved) is the assistant-side
  instance of the same unreconciled write path: copilot INSERTed an empty
  placeholder assistant row that then collided with the one
  `persist_after_chat` wrote, so the reply was never durably persisted. The fix
  was to stop diverging — *"`prepare_copilot_message` now mints a UUID instead
  of INSERTing an empty placeholder row, matching
  `chat_service::prepare_chat_dispatch`'s existing no-placeholder pattern."*
- **KYO-573** (filed `2026-08-30`, `agent-ready`, open) is watch execution's
  turn, and it is the reason the correction cannot be copied: `execute_watch_inner`
  pre-writes `"Monitor: {name}\n\n{prompt}"` while the exec config sends
  `enhanced_watch_prompt`, so the two rows differ in text. The ticket says so
  itself — *"switching to `CallerPersisted` would suppress the enhanced prompt
  from persistence while keeping the raw one"* — which makes the correct variant
  a decision about which text should persist, not a transplant of KYO-554's.

Distinct from
[../code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md](../code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md):
that rule is about *finding* the sites, and it worked here — the KYO-492 sweep
was grep-derived, complete, and included the `enterprise/kyomi-slack` site a
narrowed check would have missed. This rule is the step after the enumeration
succeeds, when every site is in front of you and each one still needs its own
answer. Distinct from
[../code-organization/close-the-class-by-making-the-wrong-call-uncallable.md](../code-organization/close-the-class-by-making-the-wrong-call-uncallable.md),
whose remedy is unavailable here on purpose: the wrong variant *must* stay
callable, because it is the right one somewhere else. Sibling of
[split-a-value-that-answers-two-questions.md](split-a-value-that-answers-two-questions.md),
which is the same incident one step earlier — that rule tells you to make the
two meanings expressible; this one is about the sites you then have to assign,
and it is where that fix leaked.
