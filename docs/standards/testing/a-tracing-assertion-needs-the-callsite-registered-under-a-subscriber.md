# A test asserting on `tracing` output must guarantee the callsite registered under a subscriber

`kyomi_test_tracing::capture_tracing()` installs its capturing subscriber with
`tracing::subscriber::set_default` (`crates/kyomi-test-tracing/src/lib.rs`) — thread-local
and RAII-scoped, so it is registered only while that one test holds the guard. `tracing`
decides, once per callsite per process, whether anyone could ever be interested in it:
`DefaultCallsite::register()` is a compare-exchange-guarded state machine that runs exactly
once, and it folds the interest of every dispatcher registered *at that instant*. If the
set is empty right then, `Interest::never()` is cached — permanently, for the lifetime of
the test binary.

So whichever test in the binary touches a given `warn!`/`error!` first decides whether that
line is observable for every test that follows. If it ran with no capture installed — a
different test, a different thread, the same suite in a different order — the callsite is
poisoned and no later `capture_tracing()` will ever see it.

The failure mode is worse than a red flake, because it splits by assertion direction. A
positive assertion ("the fallback failure is logged") goes red on the runs where the
callsite was poisoned first, which reads as flake and gets re-run. A negative assertion —
which `kyomi-test-tracing`'s own doc comment recommends `events_at` for, e.g. "no error log
contains the secret payload" — passes *vacuously* on exactly those runs, and vacuously
passing is not a signal anyone re-runs.

**Rule:** When you add a `tracing` assertion, or add a log line a test asserts on, do not
leave callsite registration to whichever test happens to reach it first. Make a
process-wide default subscriber exist before the instrumented code can run — the
`#[cfg(test)]` guard call at the top of the instrumented function, as KYO-616 shipped it —
so the interest fold is never empty. Calling `tracing::callsite::rebuild_interest_cache()`
from inside the capture helper does not fix this: the `set_default` registration it folds
over is transient, and the rebuild cannot un-cache a `Never`. And do not treat a run of
green suites as proof: a race whose probability mass sits in a narrow first-touch window
is not sampled by running the same suite again.

Both blocks below are illustrative reconstructions, not quotations: KYO-616's guard is not
on `origin/main` as of `2026-09-04`, and the function it instruments emits its warning
through a different shape in the tree today.

```rust
// WRONG — registration is left to test order. Whichever test reaches this warn!
// first, with or without a capture installed, fixes its interest for the process;
// if that first touch happened with none installed, the line is unobservable to
// every later capture_tracing() in the same binary.
pub async fn check_container_coverage(/* … */) -> Result<ContainerCoverage> {
    // …
    tracing::warn!(workspace_id, missing = %names, "catalog coverage shortfall");
}

// RIGHT — the guard travels with the instrumented function rather than with test
// discipline, so a test added later cannot reintroduce the race by forgetting it,
// and the call is erased entirely from non-test builds. The guard itself installs a
// process-wide default subscriber once, so the interest fold is never empty.
pub async fn check_container_coverage(/* … */) -> Result<ContainerCoverage> {
    #[cfg(test)]
    ensure_capture_subscriber_installed();
    // …
    tracing::warn!(workspace_id, missing = %names, "catalog coverage shortfall");
}
```

Flagged in **KYO-616** (`2026-09-03`). Cycle 1 (`10:45`) was signed clean: the flake was
already known, was correctly diagnosed as the interest cache, and had been addressed by
calling `rebuild_interest_cache()` inside a test helper — "the officially sanctioned API for
it, not a sleep-and-hope". It was still wrong. Six clean runs did not catch it; a later
10-run measurement put the real rate at 2/10, and cycle 2 (`21:18`) replaced the remedy with
the `#[cfg(test)]` guard at the top of the seven `kyomi-auth` catalog functions the diff had
instrumented. What settled it was reading `tracing-core-0.1.36/src/callsite.rs` and
`subscriber.rs` at the locked version — confirming the once-only registration, the fold over
registered dispatchers, and that `set_global_default` internally registers a `Dispatch` and
therefore genuinely joins that fold. The reviewer's own note: *"a passing test run — even
six in a row — is not evidence of correctness for a race with this shape … the fix that
actually matters was verifiable by reading the dependency's source, not by running the suite
more times."* The 25 clean runs recorded afterwards corroborate the fix; they were not what
established it.

Nearest sibling is
[nondeterministic-verdict-is-a-failing-test.md](nondeterministic-verdict-is-a-failing-test.md):
that rule covers a verdict that visibly flips between runs, and its remedy is to make the
observed window deterministic in the test. This one covers a race whose worst outcome is a
*stable green* — a poisoned callsite silently empties the capture, so a negative assertion
never fails and never gets re-run — and whose remedy is in the instrumented code, not the
test. It pairs with
[../build-toolchain/read-the-locked-dependency-source-before-resting-on-its-semantics.md](../build-toolchain/read-the-locked-dependency-source-before-resting-on-its-semantics.md),
which is how the mechanism above was established rather than guessed.
