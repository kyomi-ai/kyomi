# A check that lives inside one branch guards only that branch

A validation gets written while one input shape is in mind, so it lands inside the arm that
handles that shape — one `case` arm, one `if` with no `else`, one filter that only runs when a
field is `Some`. Every other input reaches the code past it with nothing having looked at it.
The guard is present, correct, and tested; it simply never runs for half its domain.

Three things make this survive review. First, **the unguarded arm is usually the ordinary one** —
the documented default, the `None`, the main page — so the check reads as covering the
interesting case while the case that actually runs all day is the one that skips it. Second,
**the failure is fail-open, not a crash**: exit 0 on an input that was never verified, a summary
line that is simply not printed, an event delivered to a window that should never have seen it.
Nothing is louder than a normal run. Third, **the tests were written from the same mental
model** — every fixture constructs the input the guard was written for, so the other arm has no
coverage, and the suite is green for the same reason the bug exists.

The quieter form is a dispatch with no catch-all: an `if`/`elif`/`elif` whose arms happen not to
cover every reachable state. The uncovered state does not error — it produces *nothing*, which
reads as the clean case.

**Rule:** When you add a check, enumerate the arms of the dispatch it sits in and say which arm
each reachable input takes. Prefer hoisting the check above the dispatch so there is only one
arm — a single unconditional resolve/validate beats one correct arm plus one no-op arm. If it
genuinely must stay per-arm, end the dispatch in a catch-all that speaks, and record in the code
why the remaining combinations are unreachable, rather than adding an n+1'th explicit condition
that could itself be incomplete. Then write the test for the arm you did *not* write the check
for. If every existing test constructs an input that satisfies the precondition, the guard has
no coverage at all, and neither the hole nor the fix is provable.

The shell WRONG block below is a reconstruction from the review-log entry cited underneath; the
Rust WRONG block is quoted from the pre-fix commit (`abc08537^`). The RIGHT blocks are the
shipped fixes, quoted from source.

```sh
# WRONG — the existence check lives only in the relative-path arm. An absolute
# path (the documented default, and the form in every usage example in the
# script's own header) takes the no-op arm and is never checked at all: a
# typo'd `--kyomi-repo /home/jsaon/repos/kyomi` exits 0, and the script then
# mkdir -p's a decoy tree under the typo and installs a valid-looking symlink
# inside it, while the real checkout is never touched.
case "$KYOMI_REPO_PATH" in
    /*) : ;;                                                   # absolute: unvalidated
    *)  cd "$KYOMI_REPO_PATH" || exit 1; KYOMI_REPO_PATH="$(pwd)" ;;
esac

# RIGHT — one unconditional resolve; both path forms go through the same check.
kyomi_repo_input="$KYOMI_REPO_PATH"
if ! KYOMI_REPO_PATH="$(cd "$kyomi_repo_input" 2>/dev/null && pwd)"; then
    echo "ERROR: --kyomi-repo path does not exist: $kyomi_repo_input" >&2
    exit 1
fi
```

```rust
// WRONG — the whole filter is nested inside `if let Some(ref expected_ctx)`,
// so it runs only when the component was given a context_type to filter on. On
// the main chat page that value is None, the block is skipped entirely, and
// any session's error renders in any open chat window. Note also what the arm
// never looks at even when it does run: msg.session_id.
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

// RIGHT — one default-deny gate above the body, shared by all four WS handlers,
// taking both the context type and the session id.
let event_context_type = error_event_context_type(msg.data.as_ref());

if !should_handle_event(event_context_type, msg.session_id.as_deref()) {
    return;
}
```

Real precedent — three tickets, two languages, one week:

- **KYO-584** (`2026-09-02`, `00:35`) — 🟡, blocked signing, `scripts/link-agent-skills.sh` in
  `kyomi-private`. *"`--kyomi-repo`/`$KYOMI_REPO` existence is only validated when the path is
  relative … An absolute path — which is both the documented default (`$HOME/repos/kyomi`) and
  how every usage example in the script's own header is written — takes the `/*) : ;;` no-op
  branch and is never checked for existence."* Reproduced live by the reviewer: a nonexistent
  absolute path exited 0 and created the symlink under a brand-new directory tree. The header of
  that same script claims *"it fails closed: anything it cannot verify is an error, never a
  silent success"* — and cites `empty-on-failure-must-not-look-like-a-real-result.md` as the
  shape it guards against, which it then reproduced in the one arm nobody looked at. The tests
  did not catch it because *"all 10 tests pre-`mkdir -p` the kyomi-repo dir before invoking, so
  the gap is untested as well as unhandled."* Cycle 2 (`00:45`) replaced the `case` with the
  unconditional resolve shown above, plus Test 11 for the absolute-path arm; the fix carries a
  standing comment — *"Do not reintroduce a relative-only special case here: this script never
  creates the kyomi repo, so both path forms must be validated identically"* (commit `535ab1b`,
  branch `jason/kyo-584-agent-skills`).
- **KYO-567** (`2026-08-30`, `16:51` cycle 2) — 🟡, `scripts/mark-branch-stranded.sh`'s pasteable
  release summary. *"an `if`/`elif`/`elif` with exactly three arms … and this exact state
  (`LOCAL_RENAMED=0`, `LOCAL_BRANCH_CHECKED_OUT_AT` empty, `LOCAL_BRANCH_EXISTS=1`) matches none
  of them"*, so a failed local rename printed no `Local:` line at all and the summary *"looks
  identical to a clean run with no local branch of that name"* — dropping the one fact the
  operator needed from the record documented as durable. Also untested: no case forced
  `git branch -m` to fail. Cycle 3 (`16:57`) fixed it with a catch-all `else` rather than a
  fourth condition, an in-code note that *"every other combination … is unreachable by
  construction"*, and Test 2b, which forces the failure with a pre-created conflicting local
  `stranded/<branch>` ref and asserts on the stdout summary.
- **KYO-501** (`2026-08-29`, `08:33`) — the same shape in Rust, in
  `crates/kyomi-ui/src/components/chat/chat_engine.rs`. *"Before this fix, `error` filtered only
  on `context_type` inline and never read `msg.session_id` — on the main chat page
  (`context_type = None`) the inline filter was skipped entirely, so any session's error rendered
  in any open chat window."* Fixed by routing the `error` subscription through the shared
  `should_handle_event` default-deny closure its three siblings already used (KYO-494). Note the
  arm that was skipped: not an exotic one, the default one.

Nearest sibling is
[empty-on-failure-must-not-look-like-a-real-result.md](empty-on-failure-must-not-look-like-a-real-result.md):
that rule covers a value that degrades on failure and is then read downstream as a real answer —
the check ran, its result was flattened. This one covers the input for which the check never ran,
so there is no result to flatten and the run is indistinguishable from a verified one. Distinct
from [propagate-predicate-changes-to-every-copy.md](../code-organization/propagate-predicate-changes-to-every-copy.md),
where one predicate exists at N sites that must move together; here there is a single copy, and
the defect is the input that never reaches it. Distinct from
[close-the-class-by-making-the-wrong-call-uncallable.md](../code-organization/close-the-class-by-making-the-wrong-call-uncallable.md),
where a wrong API stays legal beside a new right one; here there is only one implementation, with
a hole inside its own domain. Distinct from
[teardown-clears-the-whole-derived-state-group.md](../data-state-management/teardown-clears-the-whole-derived-state-group.md),
which is about *what* a site must clear once it runs, not whether the site runs at all. Related
to [no-guarantee-stronger-than-code-enforces.md](../comments-documentation/no-guarantee-stronger-than-code-enforces.md):
both KYO-584 and KYO-567 carried headers promising exactly the property the uncovered arm broke,
which is why neither read as suspicious.
