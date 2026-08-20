# Prove the test fails without the fix — then prove you restored the tree

A green test proves nothing on its own. It may assert a condition that holds either way: an `assert_ne!(None, None)` on a value that is `None` on both code paths, a filter that empties the collection before the assertion runs, an enum variant reachable by an early return that never touches the changed logic. Every one of those passes against the buggy code too, which means the regression it claims to prevent will ship silently.

**Rule:** Before claiming a test locks in a behavior change, revert the fix (or mutate the exact line the assertion depends on), re-run *that* test, and confirm it fails with the failure the ticket describes. Then restore from a pre-mutation copy and confirm `git diff --cached` is byte-identical to what you staged — a mutation left behind in the working tree is worse than no test. Quote the mutation and its failure output in the PR; "the tests pass" is not the claim being made. Prefer assertions that can only pass for the right reason: assert the *whole* captured log is empty rather than filtering to one level, and put an `assert!(x.is_some())` ahead of any `assert_ne!` whose `None == None` case would pass vacuously.

```bash
# 1. Break the exact line the assertion depends on
$ # edit auth_service.rs: "signup" -> "email_verification" (the pre-fix value)
$ cargo test -p kyomi-auth --locked --lib passkey_signup_verify_only_accepts_its_own_token_type
#   → MUST fail, with the cross-flow acceptance the ticket describes

# 2. Restore and prove no drift from your own testing
$ cp .backup/auth_service.rs crates/kyomi-auth/src/auth_service.rs
$ git diff --cached --stat        # identical to before the mutation
$ cargo test -p kyomi-auth --locked --lib passkey_signup_verify_only_accepts_its_own_token_type
```

Applied in almost every review in the 2026-08-01 → 2026-08-07 window, and load-bearing in several: KYO-256 mutated both auto-heal branches to echo the bogus session id back, and confirmed each `assert_ne!` duly failed with `Some(x)` on both sides rather than passing vacuously; KYO-263 mutated both guard conditions to show the fail-closed branch was covered by a real assertion rather than only by prose; KYO-222 cycle 2 re-ran the implementer's `#[serde(rename)]` mutation rather than trusting the claim; KYO-281 and KYO-282 each reverted the shipped fix to reproduce the exact panic; KYO-259 broke a matrix expression to prove `actionlint` catches it. Each of those reviews also re-confirmed the staged diff byte-for-byte after restoring.
