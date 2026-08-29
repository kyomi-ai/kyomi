# A mutation only counts if the run could have failed — prove it compiled and reached the assertion

[prove-test-fails-without-fix.md](prove-test-fails-without-fix.md) requires you to break the
code and watch the test go red. This rule is about the other half of that evidence, which is
much easier to skip: proving that the run you observed was *capable* of the outcome you are
reading into it. A green mutation run means one of two very different things — "the test does
not cover this" or "the run never executed this" — and nothing in the output distinguishes
them. A red one means "the assertion caught it" or "something upstream of the assertion blew
up first," and those are also indistinguishable from the exit code.

Four separate reviews in this window hit four different mechanisms, all producing the same
false reading:

- **The compiler never saw the change.** `#[cfg(target_arch = "wasm32")]` bodies are absent from
  the host test binary, so a host `cargo test` cannot fail because of them no matter what you do
  to them. The same applies to `#[cfg(all(test, feature = "ssr"))]` modules run without
  `--features ssr` (see [skipped-test-must-fail-loudly.md](skipped-test-must-fail-loudly.md)).
- **Cargo never rebuilt.** A clippy or check invocation that finishes in under a second against a
  stale cache reports the *previous* build's verdict.
- **The mutation ran against the wrong copy.** A script copied out of the repo to a scratch
  directory loses its `REPO_ROOT`-relative defaults; the run exercises a different configuration
  than the one under test.
- **Something upstream aborted before the assertion.** A migration-checksum guard, a connection
  failure, a marker-not-found panic — each turns the run red for a reason that says nothing
  about whether the assertion is load-bearing.

**Rule:** For every mutation, state the mechanism by which the run could have failed, and confirm
it. Before trusting a green mutation, confirm the mutated code is compiled by the target you ran
(for a `#[cfg]`-gated body, that is a different `cargo` invocation, not a different filter) and
that the specific test name appears in the output. Before trusting a red one, read the failure
text and confirm it is your assertion's own message — not a panic, not a harness guard, not a
compile error. Force a real rebuild when a lint or check run returns implausibly fast. If the run
could not have failed, say so and treat the criterion as unverified: that is a genuine finding
about the test seam, not a formality.

```bash
# WRONG — the mutation is real, the suite is green, and the conclusion is backwards.
$ # revert the fix in setup_ws_subscriptions (a #[cfg(target_arch = "wasm32")] fn)
$ cargo test -p kyomi-ui --locked --features ssr
test result: ok. 508 passed; 0 failed
# "the fix isn't covered by a test" — no: the host binary never compiled the function,
# so this run could not have failed. It is evidence about the harness, not the test.

# RIGHT — name the invocation that compiles the changed code, and check the failure text.
$ cargo clippy --locked -p kyomi-ui --target wasm32-unknown-unknown --features hydrate -- -D warnings
#   the only pass that compiles the changed handler wiring
$ touch crates/kyomi-ui/src/pages/settings/datasources.rs   # defeat the stale cache
$ cargo clippy --locked -p kyomi-ui --target wasm32-unknown-unknown --features hydrate -- -D warnings
#   now the timing is consistent with a real compile, so "clean" means something
```

Real precedent — four reviews, four mechanisms, one shape:

- **KYO-501 (`08:33`, 2026-08-29)** — the reviewer reverted the `error` handler to its pre-fix
  form and got *"all 13 `chat_engine` tests (and the full 508-test suite) stayed green."* Read
  correctly, that green is the finding: it proves `setup_ws_subscriptions` — the wasm32-only
  function containing both the bug and the fix — never compiles into the host test binary, so
  AC2 genuinely cannot be met by a host test in this workspace today. The gap was disclosed and
  ticketed (KYO-549) rather than papered over with the adjacent unit test.
- **KYO-427 (`14:20`, 2026-08-22)** — the wasm32/hydrate clippy pass *"finished in 0.75s off a
  stale cache, which would have been a false green."* The reviewer touched `datasources.rs` to
  force a genuine recompile and re-ran; only the second run is evidence that the new
  `#[cfg(target_arch = "wasm32")]` `on:click` bodies compile under the CI target.
- **KYO-414 (`03:27`, 2026-08-24)** — the first mutation attempt against the pre-fix lint script
  *"gave a false '0 failures' reading because copying the old script out to scratch broke its
  `REPO_ROOT`-relative `LINT_DIR` default."* Corrected by overriding `DISPOSAL_LINT_DIR`
  explicitly; the real result is 7 of 9 fixtures failing against the true pre-fix script. The
  review's own note: this *"nearly produced a false 'no real detection' finding."*
- **KYO-460 (`14:00` and `18:30`, 2026-08-23)** — the same mutation was run against both engines.
  SQLite failed at the documented assertion (`left: String("true") right: Bool(true)`); Postgres
  *"hit a (correct, harmless) `sqlx` migration-checksum guard on the shared dev DB instead of the
  same assertion, because the dev DB already had this exact migration applied."* Both cycles
  recorded that distinction rather than counting two engines' worth of red as two proofs.

A fifth instance is the discipline stated as a refusal: in **KYO-480 (`22:09`, 2026-08-23)** the
reviewer's `cargo test -p kyomi-server --lib` was still compiling when the file was restored, and
the entry says so plainly — *"the in-flight run is not meaningful and was not waited on further"*
— falling back to a hand-trace of a deterministic pure function, and naming that as the evidence.

Siblings, each covering one member of this family:
[anchor-source-text-markers-on-code-not-copy.md](anchor-source-text-markers-on-code-not-copy.md)
(a `marker not found` panic is red for the wrong reason),
[skipped-test-must-fail-loudly.md](skipped-test-must-fail-loudly.md) (a test that declined to
run), [run-all-targets-clippy-before-trusting-a-lint-fix.md](run-all-targets-clippy-before-trusting-a-lint-fix.md)
(the invocation omits the targets that would fail), and
[verify-lint-fixes-on-the-toolchain-that-produces-them.md](../build-toolchain/verify-lint-fixes-on-the-toolchain-that-produces-them.md)
(the lint does not exist in the binary you ran). This rule is the general question all four
answer: *could this run have produced the failure I am claiming it rules out?*
