# A green host test suite says nothing about `#[cfg(target_arch = "wasm32")]` code

`cargo test -p kyomi-ui --features ssr` builds for the host. Everything gated
`#[cfg(target_arch = "wasm32")]` — `setup_ws_subscriptions` and every WebSocket
subscription closure inside it, every browser-only handler — is not in that binary. The
compiler never parsed the bodies, so a passing run is not *weak* evidence about that code;
it is no evidence at all. Neither is the test count: adding tests to a file whose bug lives
behind the gate moves the number without moving the coverage.

The tell is a mutation that changes nothing. Revert the fix, re-run, and the suite is still
green with an identical count — which reads at a glance like "the fix is not load-bearing"
but actually means "the run never saw either version." Anyone who skips that check ships a
diff whose central claim was never tested, in a file that looks thoroughly tested.

This bites hardest on acceptance criteria written in browser terms — *a cross-session error
event must not call `set_error`* — because there is no wasm test harness in this workspace
to meet them with. The wrong response is to write a host test against something adjacent
and mark the AC met. The right one has two halves, and the second is not optional:

1. **Pull the decision the bug lives in out of the wasm-only closure** into a plain function
   gated `#[cfg(any(test, target_arch = "wasm32"))]`. That gate compiles the function for
   `cargo test` on the host *and* for the real browser target, while keeping it out of a
   plain non-test host build so nothing goes "unused". The function is then genuinely
   mutation-testable, and the wasm-only closure is left holding only wiring.
2. **Say what is still uncovered.** The subscription wiring itself — which closure is
   registered, which filter it routes through — remains untestable here. Name that gap and
   cite the ticket tracking it rather than letting the extracted test stand in for it.

Separately: the only command that compiles the gated code at all is
`cargo clippy --locked -p kyomi-ui --target wasm32-unknown-unknown --features hydrate -- -D warnings`.
The three `--features ssr` passes will not fail on anything inside the gate, so a change to
a WS handler that skips the wasm32 pass has been checked by nothing.

**Rule:** Never cite a host `cargo test`/`cargo check`/`clippy --features ssr` result as
evidence about code inside a `#[cfg(target_arch = "wasm32")]` block. Before claiming a
browser-side fix is tested, revert it and re-run: if the suite stays green, the test is
covering something else. Extract the decision into a `#[cfg(any(test, target_arch =
"wasm32"))]` function that the wasm-only closure calls, test *that* and prove the test fails
without the fix, run the wasm32/hydrate clippy pass, and state in the PR body which part of
the wiring is still unverified and under which ticket.

```rust
// WRONG — the JSON key read lives inline in the wasm32-only subscription
// closure. Nothing on the host ever compiles this line, so reading the wrong
// key ships silently and no host test can be written that would catch it.
#[cfg(target_arch = "wasm32")]
fn setup_ws_subscriptions(/* ... */) {
    ws.subscribe("error", move |msg| {
        let error_msg = msg.data
            .and_then(|d| d.get("message"))          // producer writes "error"
            .and_then(|v| v.as_str())
            .unwrap_or("An error occurred")
            .to_string();
        chat_state.set_error(error_msg);
    });
}

// RIGHT — the read is a plain function compiled for both the host test binary
// and wasm32; the closure keeps only the wiring. The doc comment names the
// gate's reasoning and its single production caller.
///
/// `cfg(any(test, wasm32))`: the only production caller is the `"error"`
/// subscription in the wasm32-only `setup_ws_subscriptions` below. Compiled
/// unconditionally it would be "unused" on a plain non-wasm32, non-test host
/// build; gating it here keeps that build clean while still compiling for
/// `cargo test` (host) and the real wasm32 target.
#[cfg(any(test, target_arch = "wasm32"))]
fn error_event_message(data: Option<&serde_json::Value>) -> String {
    data.and_then(|d| d.get("error"))
        .and_then(|v| v.as_str())
        .unwrap_or("An error occurred")
        .to_string()
}
```

Real precedent — `crates/kyomi-ui/src/components/chat/chat_engine.rs`, three reviews in
three days:

- **KYO-501** (review log `2026-08-29`, `08:33`) is where the mechanism was demonstrated
  rather than assumed. The reviewer *"reverted the `error` handler to the pre-fix inline
  `context_type`-only check and ran `cargo test -p kyomi-ui --features ssr` — all 13
  `chat_engine` tests (and the full 508-test suite) stayed green. This proves
  `setup_ws_subscriptions` (the wasm32-only function containing the actual bug and fix)
  never compiles into the host test binary, so the AC2 ("a cross-session error event is
  confirmed NOT to call set_error") genuinely cannot be met by a host test in this
  workspace today."* The diff's answer was the extraction — `error_event_context_type()`
  pulled out *"specifically to make it host-testable"* because the top-level-vs-nested JSON
  path was the one genuinely new and risky piece — plus KYO-549 filed and cited for the
  residual wiring gap across all six handlers. The same review's CI-parity list records the
  other half: the wasm32/hydrate clippy pass is *"the only pass that compiles the changed
  handler wiring."*
- **KYO-550** (review log `2026-08-30`, `02:55`) is the payoff. Because `error_event_message`
  is host-compiled, the reviewer could mutate it — reintroducing
  `.get("error").or_else(|| d.get("message"))` — and watch
  `error_event_message_does_not_fall_back_to_the_message_key` fail with
  `left: "should not be read", right: "An error occurred"`. That mutation is available only
  because the function sits outside the gate; the identical bug one level up, in the closure,
  is still unfalsifiable on the host.
- **KYO-550 rework** (review log `2026-08-30`, `21:15`) records the convention as
  established rather than ad hoc: *"`should_handle`, `error_event_context_type`, and
  `error_event_message` all carry identical `#[cfg(any(test, target_arch = "wasm32"))]`,
  consistent with the file's established pattern; the only production caller is itself
  `#[cfg(target_arch = "wasm32")]`."*
- **KYO-474** (review log `2026-08-29`, `15:05`) shows the compile half is treated as a
  standing requirement, not a special case: the wasm32/hydrate clippy invocation is run
  alongside the three `--features ssr` passes on a diff that touched no wasm-only code, on
  the principle that the ssr passes cannot speak for it.

Sibling of [wasm-only-cfg-blocks-must-compile.md](wasm-only-cfg-blocks-must-compile.md):
that rule is about *compile* coverage — a variable referenced inside a `#[cfg(target_arch =
"wasm32")]` block that isn't in scope on WASM — and its remedy is running the wasm32 check.
This rule is about *test* coverage, which the wasm32 check does not provide either: clippy
compiling the block proves it type-checks, not that the logic inside it is right. Distinct
from [../testing/cover-the-path-the-criterion-names-not-an-adjacent-one.md](../testing/cover-the-path-the-criterion-names-not-an-adjacent-one.md),
whose remedy is "assert through the entry point the AC names, extracting a seam if needed" —
here that remedy is only half-available, because the entry point is unreachable from any
harness in this workspace, so the extraction must be paired with an explicit disclosure per
[../version-control-working-tree/state-the-acceptance-criterion-you-did-not-meet.md](../version-control-working-tree/state-the-acceptance-criterion-you-did-not-meet.md).
See also [../testing/prove-test-fails-without-fix.md](../testing/prove-test-fails-without-fix.md)
and [../build-toolchain/a-cargo-run-that-compiled-nothing-verified-nothing.md](../build-toolchain/a-cargo-run-that-compiled-nothing-verified-nothing.md):
a cached run and a `cfg`-excluded run are the same defect — a green result from a compiler
that never looked at the code.
