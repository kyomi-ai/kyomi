# A test that never ran is not a passing test — make the skip fail where it matters

Two mechanisms remove a test from a run while the run still reports green. A test that skips at runtime (`let Some(pool) = connect().await else { eprintln!("SKIP: ..."); return; }`) *passes*, and the default harness captures and discards stderr for passing tests — the `SKIP:` line is invisible unless someone passes `--nocapture`/`--show-output`, which CI does not. A test behind a feature gate (`#[cfg(all(test, feature = "ssr"))]`) is not compiled at all without that feature, and the suite still exits `ok` with a healthy-looking count made up entirely of other tests. In both cases the reported total is true and the conclusion drawn from it is false, and nothing will ever surface the gap.

**Rule:** A test that can decline to run must fail loudly in the environment that is supposed to run it. Gate the skip on an explicit env var CI sets — panic naming the variable, the test, and the underlying error when it is set; skip when it is unset, so a local run without the container still works. When reporting a test count for a feature-gated suite, name the feature and confirm the specific new test names appear in the output (grep the run for them) rather than quoting a total. Never call a skip "visible" until you have watched it under the exact command CI runs.

```bash
# WRONG — passes, prints nothing, proves nothing
$ cargo test --locked --workspace --lib --bins --tests   # SKIP: line captured and discarded
test result: ok. 672 passed; 0 failed

# RIGHT — CI sets the var, so a Postgres-arm test that cannot connect fails the job
$ KYOMI_REQUIRE_POSTGRES_TESTS=1 cargo test -p kyomi-auth --locked --lib -- postgres_
#   → panics naming KYOMI_REQUIRE_POSTGRES_TESTS, the test, and the connection error
$ cargo test -p kyomi-auth --locked --lib -- postgres_   # var unset, local dev
#   → SKIP: lines, run still green
```

Flagged as 🟡 in KYO-292 (2026-08-09): `crates/kyomi-auth/src/test_pg.rs`'s module doc claimed the skip was made "visible … rather than silent, because a Postgres-arm test that always reports success without ever running the Postgres arm is worse than no test" — the intent was right and the mechanism did not deliver it, since `ci.yml`'s `cargo test --locked --workspace --lib --bins --tests …` passes neither capture flag. Fixed by having `postgres_test_pool_or_skip` panic under `KYOMI_REQUIRE_POSTGRES_TESTS=1`, which the CI job now sets alongside its `pgvector` service; both branches were then reproduced against a dead `DATABASE_URL`. The feature-gate half was flagged in KYO-278 (2026-08-08): the new `#[cfg(all(test, feature = "ssr"))]` regression tests are invisible without `--features ssr` while `kyomi-ui` still runs 161 other tests green — and the reviewer's own first pass mis-read a truncated log as "the crate compiles zero tests without ssr," a stronger claim than the evidence supported.
