# Moving *when* a row is written silently re-times every check for its existence

`if get_user_by_email(email).await?.is_some()` is never really asking whether a row
exists. It is asking a lifecycle question — *has this signup completed?*, *is this
account mid-flow?*, *did the previous step run?* — and using row existence as the
proxy. The proxy is correct at the moment it is written and carries no record of
which question it was standing in for.

A diff that moves the write earlier or later in the flow, or stops writing the row
in that state at all, inverts every one of those proxies at once. Nothing warns you.
The type is unchanged, the function name is unchanged, the call still compiles, and
**the reader that broke is invisible whether or not its file is in the diff** — a
file the diff never names gives review nothing to open, and a file the diff *does*
name is barely better, because reading the changed hunks is not what surfaces the
reader. Either way there is no identifier to grep for, so having the file open in
front of you is not a reason to skip the sweep. Worse, the characteristic symptom
is an early `return Ok(None)` or `return Ok(())`: the proxy now reads "nothing to
do here", which is exactly what an
existence check's miss branch was built to mean, so the feature becomes a silent
no-op rather than an error. Tests written against the new design pass, because they
seed the row the new design creates.

**Rule:** When a change alters *when* a row is created, deleted, or first reaches a
state — moving a write across a step boundary, deferring creation to redemption
time, collapsing two steps into one — enumerate every reader that tests for that
row or that state, and for each one write down which lifecycle question it was
using existence to answer. A reader whose question is still meaningful must be
re-keyed onto something that is still true at that point in the flow; a reader
whose question no longer exists is dead code and must be named as such, not left
in place. Re-key from the flow, not from the diff: grep the table name and the
accessor (`get_user_by_email`, `INSERT INTO users`) across `apps/`, `crates/`, and
`enterprise/`, including the state itself (`verified = false`) if that is what
moved. Then state the new invariant in a doc comment at the function the readers
now depend on, so the next move of the write has something to contradict.

```rust
// WRONG — `users` row existence stood for "this signup is still pending", and
// both guards were correct when written. Once row creation moved to
// token-redemption time no row exists during the pending window at all, so the
// first `else` became the common case: the Resend button's only real use case
// returns `Ok(None)` and sends nothing. Nothing below is wrong in isolation:
// this guard is verbatim at `890975c0^`, so it predates the phase. Whether the
// phase's own diff also edited this function is not something the repo can
// settle — see the note below on what `890975c0` squashed away.
//
// Everything from here down is verbatim from
// `git show 890975c0^:crates/kyomi-auth/src/auth_service.rs`,
// `resend_verification_service`, with the two elisions marked inline.
/// Check rate limit, look up the unverified user, and create a verification
/// token in one service call. Returns `None` if rate-limited, user not found,
/// or user is already verified.
pub async fn resend_verification_service(
    /* ... */
) -> kyomi_core::Result<Option<ResendVerificationResult>> {
    let rate = check_rate_limit(kv, ip, "register").await?;
    if !rate.allowed {
        return Ok(None);
    }

    let user = crate::user_service::get_user_by_email(db, email).await?;
    let Some(user) = user else { return Ok(None) };
    if user.verified {
        return Ok(None);
    }
    /* ... mint token, return Some(...) ... */
}
```

```rust
// RIGHT — the lookup result selects between two real branches in one shared
// gate instead of gating the whole function, so "no row" is the
// pending-signup path rather than the give-up path.
let existing_user = crate::user_service::get_user_by_email(db, email).await?;
mint_verification_email_or_notify_existing(db, email, None, frontend_url, existing_user.as_ref())
    .await
```

```rust
// RIGHT — the invariant written down where the readers now depend on it
// (crates/kyomi-auth/src/auth_service.rs, above
// mint_verification_email_or_notify_existing).
/// KYO-683 phase 1 is exactly why this has to be keyed on `existing_user`
/// rather than on some other signal: no `users` row exists between "signup
/// form submitted" and "verification link clicked", which is precisely the
/// window the Resend button is shown in. Gating on row existence (as
/// `resend_verification_service` used to) makes that whole window a silent
/// no-op — the row lookup returns `None` and there is nothing to resend to.
```

Flagged as 🔴 (Correctness / Missing Error Context) in **KYO-683 phase 1**
(`docs/review-logs/2026-09-10.md`, 15:57, finding 2). The phase moved `users`-row
creation from signup-start to verification-token redemption
(`establish_verified_account`). `resend_verification_service` gated on
`get_user_by_email`, falling out through `let Some(user) = user else { return
Ok(None) };` — present verbatim at `890975c0^` — so the Resend button in
`crates/kyomi-ui/src/pages/auth/login.rs` (`resend_action`) became a no-op for its
only real use case, with no test covering that path in the diff cycle 1 reviewed.
(The two tests named below are not a counter-example: they arrived with the cycle-2
fix and are present in `890975c0`, so "no coverage" describes what cycle 1 had in
front of it, not the merged commit.) The reviewer established the breakage by grep
rather than inference: no production path writes a `verified = false` row any
more — every non-test `create_user` call now passes `true` — so the state the
check selected for had ceased to exist. Cycle 2 (16:41) fixed it by extracting
`mint_verification_email_or_notify_existing`
(`crates/kyomi-auth/src/auth_service.rs:514`) that both `signup_start_service` and
the rewritten `resend_verification_service` (`:2570`) call, "keyed on a fresh
`get_user_by_email` lookup rather than a stale row-existence assumption", with
`resend_verification_mints_a_token_for_pending_signup_and_creates_no_user_row` and
`resend_verification_mints_no_token_for_verified_user` pinning both branches.
Both cycles squash-merged as `890975c0` (PR #511 was one branch commit,
`5c3011ae`, landing as one commit on `main`), which is why the pre-fix guard is
quoted from `890975c0^` above rather than from a separate pre-fix commit. It is
also why one thing cannot be established from this repo at all: whether the phase's
own diff edited `resend_verification_service`, or only changed the world underneath
a function it left alone. The guard being verbatim at `890975c0^` argues the
latter; the finding quoting `return Ok(())` argues the former, because that return
shape exists only under the post-phase `-> Result<()>` signature — at `890975c0^`
the function returns `Result<Option<ResendVerificationResult>>` and the guard reads
`return Ok(None)`. The code blocks above follow the source over the log, per
[../comments-documentation/quote-the-artefact-not-the-log-that-quotes-it.md](../comments-documentation/quote-the-artefact-not-the-log-that-quotes-it.md);
the log's `Ok(())` is named here rather than quietly dropped so the next author does
not re-derive it from the log and reinstate it. The intermediate state that would
decide it exists in no commit, so neither reading can be settled — and the rule does
not depend on which is true.

The same review's 🟢 #5 is the second half of the shape and the reason the
enumeration is the rule: the `/verify-email` page and its `verify_email` server fn
were left *fully orphaned* by the same lifecycle change — nothing emails a link to
them any more. One reader of the removed state silently mis-answered, another
became unreachable; the mis-answering one sat in
`crates/kyomi-auth/src/auth_service.rs` — a file the diff had open, and the first
entry on that review's own Files line — and the unreachable one in
`crates/kyomi-ui/src/pages/auth/verify_email.rs`, which the diff never named at all.
Having the file open is not what found the first of them: the reviewer got there by
grepping the invariant — every non-test `create_user` call now passes `true` — not
by reading the hunks. A re-time produces both outcomes at once, and only enumerating
the readers distinguishes which is which.

Distinct from
[audit-write-sites-when-tightening-constraint.md](audit-write-sites-when-tightening-constraint.md):
that rule sweeps *write* sites when a column constraint tightens and the failure is
a loud `INSERT` error; this one sweeps *read* sites when a write moves in time and
the failure is silence. Distinct from
[../code-organization/propagate-predicate-changes-to-every-copy.md](../code-organization/propagate-predicate-changes-to-every-copy.md),
where the predicate is duplicated and one copy goes unedited — here the broken
predicate has exactly one copy and was correctly left alone; its *meaning* changed
underneath it. Closest neighbour is
[../code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md](../code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md),
and the remedy rhymes — enumerate from the producer, not the diff — but that rule is
triggered by a renamed or reshaped identifier, something a grep can chase. Nothing
was renamed here, which is why the sweep has to be keyed on the invariant rather
than on a symbol. See also
[../error-handling/empty-on-failure-must-not-look-like-a-real-result.md](../error-handling/empty-on-failure-must-not-look-like-a-real-result.md):
a re-timed existence check is how an `Ok(None)` comes to mean "nothing to do" for
a case that very much had something to do.
