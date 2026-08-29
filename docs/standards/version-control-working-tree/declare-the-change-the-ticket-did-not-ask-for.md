# Declare the change the ticket did not ask for

The mirror image of
[state-the-acceptance-criterion-you-did-not-meet.md](state-the-acceptance-criterion-you-did-not-meet.md).
That rule is about a diff that does *less* than the ticket asked. This one is about a diff
that does *more* — a generalization past the three cases named, a neighbouring call site
moved on the way past, a behaviour change that nobody chose but that falls out of an
implementation decision. Extending scope is frequently the right call and is not the defect.
Extending it silently is.

Silent extension is hard to catch from inside the diff, because the extra change is usually
the *good* part of it. It compiles, its tests pass, it reads as competent engineering, and
nothing about it looks like a question. The only way to find it is to hold the diff against
the ticket and ask, of each hunk, "who asked for this?" — the same audit nobody performs by
default that the sibling rule is written from.

The subclass that actually hurts is the one that is not a feature at all: a behaviour change
that was never decided, only implied. Choosing to store the raw form of a value rather than
the annotated form is a storage decision; that it also changes what the model reads back on
the next turn is a consequence, and consequences do not get tests, PR-body lines, or anyone's
agreement, because nobody noticed there was a decision to make.

Across this window the reviewers treated an out-of-scope extension as a finding on exactly
one axis, and it was not size, risk, or whether the extension was in the ticket. It was
whether the diff itself said so. Every extension carrying its own in-code justification — a
doc comment naming the invariant that makes the generalization safe, a comment explaining why
the second call site had to move too — was accepted *as an extension* in the pass it appeared
in, including in a review that was not signed for three unrelated reasons. The one that
arrived unannounced was written up as a finding, and the remedy asked for was a decision and
a sentence, not a revert.

**Rule:** if your diff changes behaviour the ticket did not ask for, say so in two places: a
doc comment at the site, stating the reasoning that makes the extension correct, and a line in
the PR body naming what else changed and what it costs. Do it at PR-body time, not when a
reviewer asks. For a behaviour change that fell out of an implementation choice rather than
being chosen, decide it explicitly — pin the new behaviour with a test, make the paths that
disagree agree, or file the residue as its own ticket; an untested consequence is not an
accepted tradeoff, it is an unnoticed one. And when the ticket carries an explicit constraint
("do not change the copy", "don't touch the schema"), a change that lands inside it needs
sign-off, not silent application — even when the change is an obvious improvement, and even
when nobody is present to give it. Surface it as an open question in the PR body and leave it
unapplied.

```
WRONG — the extension is real, correct, and unmentioned:

## Summary
- Persist the user message at dispatch instead of after the agent loop
  completes, so an interrupted run can't lose it.

(The diff also changes *what* gets stored — the raw message rather than the
metadata-prefixed form — so from turn 2 onward the reloaded history has lost
each past turn's source/local-time annotation. No test, no mention, no
decision: it is a side effect of where the write moved to.)

RIGHT — same diff, the consequence named and decided:

## Summary
- Persist the user message at dispatch instead of after the agent loop
  completes, so an interrupted run can't lose it.

## Also changed
- The stored form is now the raw message, not `build_metadata_prefix`'s
  `[source: web, user_local_time: …]` annotation, so past turns no longer
  carry per-turn source/time context into rebuilt LLM history; the current
  turn still gets it live. Decided deliberately rather than left as a side
  effect: the `skip_ai` and AI paths now agree on storing the raw message,
  and the LLM-context path's own missing prefix-strip is filed as KYO-506
  rather than folded in here.
```

Five entries, five tickets, three days — one silent, four declared:

- **KYO-492** (review log `2026-08-23`, `00:58`, finding #3 of four: 🟡, category "Quick &
  Dirty / undocumented scope") — the silent one. Beyond the durability fix in
  `prepare_chat_dispatch` (`crates/kyomi-auth/src/chat_service.rs`), the diff changed what
  the web chat path stores, and therefore what `get_agent_messages` hands back to the model
  on reload. The reviewer:
  > This is untested and undiscussed as a deliberate behavior change — it's a side effect of
  > choosing to store the raw form. Confirm this is an accepted tradeoff (and say so in the
  > PR) or preserve the annotation across turns.

  The remedy asked for was disclosure or a decision, not a revert. That entry was not signed,
  but for the two 🔴 blockers alongside it ("primarily the workspace does not build as
  staged") — this 🟡 did not cost the cycle on its own. It was resolved by decision rather
  than disclosure: the version that shipped (review log `2026-08-24`, `04:55`, clean) has
  "both `skip_ai` and AI paths now agree on storing the raw message," with the residual
  LLM-context gap filed as KYO-506 and confirmed out of scope in the review.
- **KYO-517** (`2026-08-24`, `11:20`, Clean/approved on the initial pass, no cycle) —
  accepted. `connection_step_satisfied_from`
  (`crates/kyomi-ui/src/pages/settings/datasources.rs`) was generalized from the three
  provider/auth-mode pairs the ticket named to a registry-derived dispatch over the existing
  `*_oauth_source` functions. Broader than the brief and approved in the same pass, because a
  doc comment on the function stated the invariant that made the generalization safe. The
  reviewer's note is the rule in one line:
  > Well-executed generalization beyond the literal brief, with the risk of that
  > generalization pre-empted by an accurate doc comment rather than requiring reviewer
  > pushback — a good pattern for future "I generalized your ask" diffs to follow.
- **KYO-440** (`2026-08-24`, `13:19`, cycle 1: 0 🔴, 3 🟡, not signed) — the *extension* was
  accepted; none of the three findings was about it. A new `oauth_recovered` signal, outside
  the literal brief, was justified in the diff by a claim about `<For>` not re-invoking a
  retained key's view function. The reviewer checked that claim against the `tachys` version
  this workspace's `Cargo.lock` actually resolves (`0.2.18`, "not assumed") rather than
  taking it, and it held:
  > The claim is correct; `oauth_recovered` is a justified addition, not scope creep.

  Declaring an extension invites the check; it does not exempt you from it, and it does not
  immunise the rest of the diff.
- **KYO-428** (`2026-08-23`, `05:20`, Clean/approved) — accepted.
  `save_datasource_credentials` (`crates/kyomi-ui/src/server_fns/datasources.rs`) was moved
  to the JSON input codec although only its sibling `create_datasource_modal` was named, and
  was *"judged justified, not scope creep"* because leaving it behind
  > would make decoded shape path-dependent on which of two call sites sent the map

  — an asymmetry the diff's own comment on that function already described.
- **KYO-435** (`2026-08-22`, `13:35`, Clean/approved — 0 🔴, 0 🟡, 2 🟢) — the
  explicit-constraint case, adjudicated both ways in one review. The ticket said do not change
  the copy. Moving paragraph boundaries to promote "Request access" to a button label was
  judged presentation and *"inherent to the ticket's own ask, not scope creep"*; capitalizing
  a caption's first word — a smaller and more obviously correct edit — was flagged 🟢 as
  > technically a copy change per the ticket's own "do not change copy" constraint — flag for
  > explicit sign-off rather than apply silently.

  Cycle 2 (`14:05`) records the right response to that flag, and it is not "apply it anyway":
  the nit was *"deliberately left unactioned, correctly … Surfaced as an open question in the
  PR body instead of decided unilaterally — the right call."* Under a stated constraint,
  defensibility is not the bar; disclosure is.

See also [every-fix-ships-with-a-test.md](../testing/every-fix-ships-with-a-test.md): a
behaviour change you decided to keep is a behaviour change, and needs the same test as one you
were asked for. And
[a-deferral-ticket-is-not-always-enough.md](a-deferral-ticket-is-not-always-enough.md) for the
opposite move — the change you declined to make, and when a ticket is not a good enough
answer for it.
