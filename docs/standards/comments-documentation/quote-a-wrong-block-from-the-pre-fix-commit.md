# Quote a WRONG block from the pre-fix commit, don't reconstruct it from memory

A standards rule is usually written *after* the defect was fixed — often in the same run
that fixed it. So by the time the author reaches for a WRONG example, the wrong code no
longer exists in the working tree. The natural move is to retype it from memory. That
retyped block is a **reconstruction**, and reconstructions are the one part of a standards
document that no existing check can catch.

Every other accuracy rule in this section keys off something that resolves. A fabricated
type is caught by grepping for it
([verify-every-identifier-in-a-doc-code-example.md](verify-every-identifier-in-a-doc-code-example.md)).
A misquoted incident is caught by opening the log
([verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md)).
A stale `file:line` is caught by following it
([anchor-a-citation-to-a-symbol-not-a-line-number.md](anchor-a-citation-to-a-symbol-not-a-line-number.md)).
A reconstruction defeats all three at once: every identifier in it resolves, it cites no
incident of its own, and it deliberately points at code that is gone. It can be
syntactically perfect, name only real symbols, and still fail to exhibit the defect the
rule exists to describe — and nothing in the repository disagrees with it.

That is the specific failure. Not "the example is fabricated" but **"the example is
plausible and demonstrates the wrong thing"**, in the one document class whose entire
purpose is to be copied.

**Rule:** When a WRONG block depicts code that was fixed, quote it verbatim from the
pre-fix commit — `git show <fix-sha>^:<path>` — rather than retyping it. Mark any cut with
an explicit elision comment instead of paraphrasing the removed lines away. If you
genuinely cannot recover the original (it was corrected before it was ever committed, so
there is no `^` to quote), **label the block a reconstruction in the file itself**, and
then re-read it asking the one question a grep cannot: *does this code actually exhibit the
defect?* Check the guarded variable, the direction of the comparison, and which branch the
check sits in — those are what drift, and each of them still compiles.

```rust
// WRONG — a reconstruction of the pre-fix chat error subscription. Every
// identifier here is real and it compiles, but it guards on the *event's*
// context_type being Some. The real bug guarded on the component's own
// filter prop. The shape survives the retyping; the subject of the check
// does not, and that is the whole point of the rule it was illustrating.
if let Some(ct) = event_context_type {
    if !context_type_matches(&ct) {
        return;
    }
}

// RIGHT — the same block quoted from abc08537^ (the commit before
// "fix(chat): filter the error WS handler by session_id, not just
// context_type", #429), crates/kyomi-ui/src/components/chat/chat_engine.rs.
// The guard is on `ctx_type` — the component's filter prop — so the whole
// check is skipped whenever that prop is None, which is the defect.
let ctx_type = context_type.try_get_value().flatten();
if let Some(ref expected_ctx) = ctx_type {
    let event_context_type = msg
        .data
        .as_ref()
        .and_then(|d| d.get("context_type"))
        .and_then(|v| v.as_str());

    if event_context_type != Some(expected_ctx.as_str()) {
        return;
    }
}

// ... error_msg extracted here ...

chat_state_error.set_error(&error_msg);
```

The elision comment above is doing real work: it marks that the unrelated `error_msg` string
extraction was cut, so a reader diffing the block against `abc08537^` finds a gap that is
declared rather than a discrepancy that is not. Note what the comment does *not* say — how
many lines were removed. A count is the part that rots when the quoted commit is
re-examined or the elision is later widened, and it buys the reader nothing they cannot see;
name what was cut instead, per
[name-the-invariant-not-a-count.md](name-the-invariant-not-a-count.md).

Precedent — **KYO-590**, mined standard *"a check that lives inside one branch guards only
that branch"* (review log `2026-09-02`). Its PR, **#455, is still open at the time of
writing** — this precedent is in flight, not settled, and the file is not on `main`; check
its state before relying on it. The rule's own subject matter is on display here: between
the review cycles quoted below and this writing, that document was retitled from *"a check
in one arm does not guard the others"* with no corresponding review-log entry, so a citation
written from the log alone would already name a heading that no longer exists. Cycle 1
signed with one 🟢 on exactly this:
the Rust WRONG block *"inverts which side of the check the actual pre-fix bug gated on: the
real code (commit `abc08537`, pre-fix) guarded on `ctx_type` — the component's own filter
prop being `Some` — not on the event's `context_type` being `Some`."* The reviewer noted
the file already labelled the block a reconstruction, so it was not a misattribution — but
*"a reader diffing it against the real pre-fix commit would find the guarded variable
swapped."* Cycle 2 replaced it with the verbatim `abc08537^` quote and one marked elision,
and updated the framing sentence to say which blocks are quoted and which are
reconstructions. The nit was acted on rather than deferred, which invalidated the review
signature and cost a full re-review — cheaper than shipping it, and cheaper still if the
block had been quoted the first time.

A second instance in the same window: the review of
[../security/prove-a-flagged-secret-is-fake-before-suppressing-the-scanner.md](../security/prove-a-flagged-secret-is-fake-before-suppressing-the-scanner.md)
(`2026-09-02`, 🟡, filed as *technical-accuracy-in-standards-doc*) found two factual errors
about the fixture that document cited as its evidence — the same class of drift, in the
citation rather than in the code block.

Nearest sibling is
[a-resolving-identifier-is-not-a-verified-claim.md](a-resolving-identifier-is-not-a-verified-claim.md),
which makes the general form of this argument: an existence check answers "is this name
real?" and says nothing about the proposition attached to it. This rule is the case where
there is no proposition to check *against the tree at all* — the code being described was
deleted by the very commit the rule exists to discuss, so neither the grep nor a reading of
current `HEAD` can adjudicate it, and the only adjudicating artefact is `<fix-sha>^`. That
is what makes it worth its own file rather than an example inside that one: the remedy is a
different command, not a more careful application of the same one. Distinct from
[verify-every-identifier-in-a-doc-code-example.md](verify-every-identifier-in-a-doc-code-example.md),
which is about a name in an example that resolves nowhere and whose remedy is a grep; here
every name resolves and the *semantics* are wrong. Distinct from
[comment-must-describe-this-code.md](comment-must-describe-this-code.md), which is about a
comment drifting from the code beside it; a WRONG block is *supposed* to differ from
current code, so that rule cannot flag it. Distinct from
[stale-doc-comment-is-a-defect.md](stale-doc-comment-is-a-defect.md) for the same reason:
staleness is the intended state here, and correctness has to be judged against a commit
rather than against `HEAD`. The prose analogue is
[verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md)
— same discipline of opening the cited artefact, applied to the narrative rather than to
the code block.
