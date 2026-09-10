# Anchor a source-text marker on the symbol's name, not on the qualifiers around it

[anchor-source-text-markers-on-code-not-copy.md](anchor-source-text-markers-on-code-not-copy.md)
establishes that an `extract_between` marker must point at structure the
regression cannot delete — "a `fn` signature" being its headline example. That
rule is right and this one does not weaken it. But "anchor on a `fn` signature"
is not the same instruction as "anchor on the whole `fn` signature", and the
difference is where the guard actually breaks.

A function's *name* is its identity. Everything else in the signature —
`pub`, `async`, `unsafe`, `const`, the parameter list, the return type — is
churn. Each of those can change for reasons that have nothing to do with the
property the guard protects, and when one does, `extract_between` panics with
`end marker not found` instead of asserting. That is a strictly worse failure
than a red assertion: it names a string rather than a defect, it fires on every
PR in the queue regardless of what the PR touched, and — because the panic
happens during extraction — it takes the guard's real assertions offline at the
same moment, so the property they protect is now unguarded *and* nobody can tell.

**Rule:** put the smallest thing that identifies the symbol in the marker —
`"fn submit_panic_report("`, `"fn DatasourcesPage("` — and leave the qualifiers
and the parameter/return types out. When two markers could match (a call site
before the definition), prefer a different *structural* anchor over adding
qualifiers to disambiguate; the leftmost-match property, not the qualifier, is
what makes the slice correct. State in a comment which symbol the marker is
pinned to and why that one, so the next signature change has something to
notice.

```rust
// WRONG — `async` is a qualifier, not identity. KYO-686 made this function
// synchronous for an unrelated, correct reason, and the guard went off.
extract_between(
    PANIC_OVERLAY_SRC,
    "fn build_panic_context(",
    "async fn submit_panic_report(",
);

// ALSO WRONG — pins the return type too, so changing the error type of a
// function this test does not care about turns the guard into a panic.
extract_between(
    SRC,
    "pub async fn get_workspace_slack_status() -> Result<crate::types::WorkspaceSlackStatus, ServerFnError> {",
    MOD_TESTS_MARKER,
);

// RIGHT — the name and its opening paren, nothing else.
extract_between(
    PANIC_OVERLAY_SRC,
    "fn build_panic_context(",
    "fn apply_report_success(",
);
```

## Precedent

**KYO-722** is the worked example, and it is the expensive kind. `ee6e7d5c`
(PR #500) added two source-guard tests to
`crates/kyomi-ui/src/utils/feedback_context.rs` anchored on
`"async fn submit_panic_report("`. Four commits later `f3ac13dd` (PR #502,
KYO-686) changed that signature from
`async fn submit_panic_report(window: &web_sys::Window, …)` to a synchronous
`fn submit_panic_report(` — a deliberate and correct fix, because awaiting there
would have polled the executor the panic had already poisoned. Nothing about the
guarded property changed. But the marker no longer matched, so both tests
panicked during extraction, `cargo test -p kyomi-ui --lib` went to
`567 passed; 2 failed`, and **every open PR failed the Clippy CI job regardless
of its contents** — confirmed on PR #505 (docs and scripts only) and PR #507
(auth only), neither of which touches either file. `/merge-sweeper` could merge
nothing that day. Because the guard is a source-string comparison, rerunning CI
could never clear it.

The marker obeyed the sibling rule — it *was* a `fn` signature, not UI copy —
and still broke, because it carried a qualifier the regression it guards has no
relationship to.

This is not a single slip. A sweep of `extract_between`-style markers at
`c964db91` found **twelve** carrying `pub`/`async` qualifiers across
`kyomi-auth` and `kyomi-ui`, four of which pin the *entire* signature including
the return type. Each is the same latent trip-wire, waiting on an unrelated
refactor.

## Relationship to the nearest sibling rule

[anchor-source-text-markers-on-code-not-copy.md](anchor-source-text-markers-on-code-not-copy.md)
answers **what kind of thing** to anchor on: code structure, never prose or the
statement the regression rewrites. This rule answers **how much of that
structure** to include once you have picked it: the identifying name, and no
qualifier that can change without the guarded property changing. A marker can
satisfy the first rule and violate this one — KYO-722 did exactly that, which is
why the two are separate files.

Both are downstream of
[prove-test-fails-without-fix.md](prove-test-fails-without-fix.md)'s
mutation requirement, and the sibling rule's warning applies here unchanged:
when you mutation-test a source guard, check *how* it failed. A
`marker not found` panic means the anchor is wrong even though the run was red.
