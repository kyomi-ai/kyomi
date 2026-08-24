# A value that answers two questions must be split, not guarded

One signal, one struct field, one map key — read in two places to answer two *different*
questions. "Which row is currently busy?" and "is this particular attempt still wanted?"
sound like the same question until a writer legitimately answers one of them and silently
answers the other too.

The trap is that the first fix always looks like it worked. Someone finds the writer that
broke it and adds a guard there; the reported repro stops reproducing; the tests go green.
But the guard is an enumeration, and the enumeration has to stay complete forever — against
every writer that exists today and every one added later, including ones in a different
component that never heard of the invariant. **The tell that you are in this trap is a
review cycle that closes one manifestation and immediately surfaces another with the same
root cause.** When that happens twice, stop adding guards.

Splitting is cheap and it closes every vector at once, including the ones nobody has found
yet: give each question its own value, owned at the scope that actually knows the answer. A
per-attempt token or a row-private `bool` cannot be overwritten by a sibling row, by a
shared listener, or by a modal that was written two months earlier — not because a guard
forbids it, but because there is nothing there to overwrite. For a struct field carrying two
signals, the same move is an enum: two variants make "I want the id but I did *not* persist
it" expressible, so a caller cannot silently opt into the meaning it didn't want.

**Rule:** Before adding a guard to protect a shared value from a writer, ask what questions
that value is being read to answer. If it is more than one, split it — one value per
question, each owned by the scope that can answer it — rather than threading a guard through
every writer. A mechanical rename or a widened guard that leaves the overload in place is
not a fix: it compiles, it passes, and it relocates the bug. When you do split, pin the split
with a test that fails if the two are recombined; a future edit that collapses the enum back
into an `Option`, or reads the shared signal from the private gate again, must not be silent.

```rust
// WRONG — one signal answers "which row's button shows Connecting…" and
// "is this row's popup monitor still wanted". Any writer of the first
// silently cancels the second, with no outcome reported at all.
let still_connecting = move || {
    oauth_connecting.try_get_untracked().flatten().as_deref() == Some(row_id.as_str())
};
// ...and each newly-discovered writer gets its own guard, forever.

// RIGHT — the monitor gets a private token; the shared signal keeps only
// the UI question it was always good at.
let (connect_attempt_live, set_connect_attempt_live) = signal(false);
let still_connecting = move || connect_attempt_live.get_untracked();
```

Two tickets, one shape:

- **KYO-440** (review log `2026-08-24`, cycles 1–4 at `13:19` / `13:49` / `14:22` / `06:15`)
  — `oauth_connecting: ReadSignal<Option<String>>`
  (`crates/kyomi-ui/src/pages/settings/datasources.rs:609`) was read both as the row's
  "Connecting…" indicator and as the popup monitor's `still_connecting` gate. Cycle 1 found a
  same-row re-click leaking a live monitor; cycle 2 found that *another* row's click flipped
  the signal, so row A's monitor observed `still_connecting() == false` and self-stopped
  without ever calling `on_outcome` — the closed-without-message/timeout recovery this ticket
  exists to add silently no-opped; cycle 3 found the fix (widen the click guard) bought
  nothing, because the settings modal's `ModalOAuthStatusPanel::start_connect` is an
  independent OAuth flow whose `postMessage` the list-level listener also clears the signal
  on — while the widened guard locked out every other row's button as collateral. Cycle 4
  implemented the split (a row-private, wasm-gated `connect_attempt_live` as the sole input
  to `still_connecting`) and all three vectors closed together. The reviewer's cycle-2 note is
  the rule in one line: *"Those are two different questions being answered by one signal."*
- **KYO-492** (🔴 API Design, review log `2026-08-23` `00:58`; shipped `2026-08-24` `04:55`)
  — `AgentExecutionConfig`'s user-message-id field did double duty: it tagged the id for
  WebSocket streaming *and* signalled "already durably persisted, skip re-persisting". The
  Slack call site (`enterprise/kyomi-slack/src/routes.rs`) only ever wanted the tag, so a
  find-and-replace rename would have compiled and silently stopped persisting every Slack
  user message — strictly worse than the bug the ticket was fixing. Resolved by splitting
  into `UserMessagePersistence::{AdapterPersists, CallerPersisted}`
  (`crates/kyomi-agent/src/adapter.rs`), with tests written specifically to fail if a future
  edit recollapses it into a bare `Option<String>`.

Sibling of
[teardown-clears-the-whole-derived-state-group.md](teardown-clears-the-whole-derived-state-group.md):
that rule is about several signals derived from one input and a teardown that clears only
some. This one is the inverse — one value standing in for several meanings, where no amount
of clearing helps. See also
[enumerate-consumers-from-the-type-not-from-the-diff.md](../code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md),
which is how you find the writers in the first place; this rule is what to do once you have.
