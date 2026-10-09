# A test's verdict must not depend on the ambient environment

A test that reads `std::env::var` — directly, or through a lazily-initialised config that the
production code reads that way — has an input nobody passed it. It is perfectly deterministic
on any one machine and gives a different answer on the next, which is the worst of both worlds:
not unstable enough to be spotted as flaky, not stable enough to mean anything. The suite
reports a number, and the number is a property of the shell that invoked `cargo`.

Both polarities are live in this repo, and they fail in opposite directions:

- A test that needs a variable **present** passes in CI, where the workflow sets it, and fails
  for every developer who runs the full suite locally.
- A test that asserts the **absent** case (`…_returns_none_without_env`) passes in CI, where
  nothing is set, and fails for every developer whose shell has sourced the project's `.env` —
  which starting the local dev server requires them to do.

The second is the expensive one, because the cost lands on reviewers rather than on the author.
A red test in a crate the diff barely touches is indistinguishable from a regression until
somebody spends the time to prove otherwise, and that time is spent again by the next reviewer,
and the one after that.

**Rule:** A test asserts on values it controls. Do not let the process environment reach an
assertion.

1. **Prefer a parameter over a lookup.** Push `env::var` to the edge — one function that reads
   the environment and hands the value inward — and test the function that takes the value. A
   resolver you can call with a `None` is testable; a `LazyLock` built from `env::var` at first
   use is not, because the first use may be an unrelated test in the same process and a later
   `remove_var` changes nothing.
2. **If the wiring itself is under test, set and clear the variable in the test** rather than
   inheriting it — remembering that the environment is process-global, so such tests must be
   serialised, not merely written.
3. **If the value genuinely has to come from outside, make the requirement explicit and loud**,
   the way `KYOMI_REQUIRE_POSTGRES_TESTS` does (see
   [skipped-test-must-fail-loudly.md](skipped-test-must-fail-loudly.md)) — a named variable a
   runner sets on purpose, never an inherited one. Setting it in a CI step and nowhere else is
   a compensating control, not a fix; say so and file the fix.

And when a suite comes back red on a machine, "environmental" is a *hypothesis*. Reproduce it
(`env -u VAR …`, or by setting the variable) before recording it as a known flake, and make
sure it is somebody's ticket — an unowned one charges its diagnosis fee to every reviewer who
meets it next.

```rust
// WRONG — quoted verbatim from crates/kyomi-auth/src/stripe_config.rs. The
// lookups go through a `LazyLock<StripePrices>` built from `env::var`, so the
// real assertion is "no STRIPE_* variable is set in whatever shell invoked
// cargo". True in CI, false in any dev shell that sourced .env.
#[test]
fn test_all_lookups_return_none_without_env() {
    assert!(get_cloud_price_id().is_none());
    assert!(get_ai_bundle_price_id().is_none());
    assert!(get_analytics_bundle_price_id().is_none());
}

// RIGHT — a sketch of the shape, not existing code. The environment is read in
// one place; the decision is a pure function of what that read returned, so the
// test supplies its own input and means the same thing on every machine.
fn prices_from(lookup: impl Fn(&str) -> Option<String>) -> StripePrices { /* … */ }

#[test]
fn an_absent_or_empty_price_id_both_resolve_to_none() {
    assert!(prices_from(|_| None).cloud_monthly.is_none());
    assert!(
        prices_from(|_| Some(String::new())).cloud_monthly.is_none(),
        "an empty variable is not a price id"
    );
}
```

Real precedent — two tests, three reviews, three days, and one diff that produced two different
totals depending on who ran it:

- **KYO-257** (`2026-09-02` log, `KYO-257: un-quarantine has_hsts_header_when_not_demo`, 🟢) —
  `apps/server/tests/contract_health.rs::has_hsts_header_when_not_demo` was un-quarantined by
  adding `FRONTEND_URL: "https://ci-test.kyomi.invalid"` to the `cargo test` step in
  `.github/workflows/ci.yml`. The reviewer: *"`FRONTEND_URL` is set only for the CI step, not
  for local `cargo test --workspace`; a dev running the full suite locally without it will
  still see this test fail, reproducing (in miniature) the original quarantine condition."*
  Accepted as the ticket's sanctioned option, mitigated only by a doc comment naming the exact
  local invocation.
- **KYO-271 / KYO-368** (`2026-09-03` log, the two
  `KYO-271 / KYO-368: extract kyomi-auth … SQLite test fixtures` entries) — the same staged
  diff, reviewed twice on the same day, reported **774 passed / 0 failed** in one session and
  **773 passed / 1 failed** in the other. The entire delta was
  `stripe_config::tests::test_all_lookups_return_none_without_env`, failing "because this
  shell's `.env` populates `STRIPE_*` vars". Each reviewer had to establish independently that
  a red test had nothing to do with a diff about SQLite fixtures.
- **KYO-636** (`2026-09-04` log,
  `KYO-636: tracing interest-cache race guard consolidation (independent re-verification)`) —
  the same test, the same cause, a third time, a day later. The reviewer reproduced it, then
  had to run the targeted suite three times under `env -u STRIPE_*` to get a usable result.
  Tracked as KYO-357, open throughout.

Distinct from
[nondeterministic-verdict-is-a-failing-test.md](nondeterministic-verdict-is-a-failing-test.md):
that rule covers a verdict that flips **between runs on one machine**, caused by a race the app
does not guarantee against, and its check is three consecutive runs. Three consecutive runs
will never catch this one — it is perfectly reproducible per machine and diverges only
**across** machines. Distinct from
[skipped-test-must-fail-loudly.md](skipped-test-must-fail-loudly.md): there the test does not
run at all and the suite still reports green; here it runs and returns the wrong answer
confidently. Same instinct as
[../build-toolchain/verify-lint-fixes-on-the-toolchain-that-produces-them.md](../build-toolchain/verify-lint-fixes-on-the-toolchain-that-produces-them.md),
applied to a test's environment instead of a lint's toolchain: a verdict belongs to the
configuration that produced it, and a verdict from a different configuration is a different
claim.
