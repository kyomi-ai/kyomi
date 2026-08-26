# When a fix rests on a dependency's semantics, read that dependency's source at the locked version

A surprising share of this repo's frontend fixes are not really changes to our code — they
are bets on someone else's. *Does a `Memo` notify subscribers when the recomputed value
compares equal? Does a branch not taken re-subscribe to the signals it didn't read? Does
dropping a `gloo_timers::callback::Interval` clear the underlying browser timer
synchronously, or at some later point? Do `String` and `&'static str` produce byte-identical
responses?* If the answer is what you assumed, the fix works. If it isn't, the fix is a
no-op that ships green — every test passes, because the tests assert our code's shape, not
the library's behaviour.

Paraphrasing a crate's docs from memory is where this goes wrong, and it goes wrong quietly:
"a memo is cached" and "a memo only notifies on change" are both true statements about
different things, and only the second one makes collapsing a state enum into a unit variant
load-bearing rather than decorative. The same hazard applies to platform specs — a doc
comment in `crates/kyomi-ui/src/utils/oauth_popup.rs` asserted that `window.opener` "is
specified to be either null or a WindowProxy, so once null and undefined are excluded there
is nothing else it can be." The IDL attribute has an unrestricted setter; the spec does not
say that.

The version matters as much as the crate. `Cargo.lock` — not the caret range in
`Cargo.toml` — is what your build compiled and what the behaviour claim is about.

**Rule:** if a change's correctness argument is a sentence about what a dependency does,
open that dependency's source at the version `Cargo.lock` resolves, and cite crate, version
and the item you read (`reactive_graph-0.2.14`, `src/computed/inner.rs::update_if_necessary`)
in the PR body or the code comment. For the registry, that is `~/.cargo/registry/src/`; for
`kyomi-connect` and `chartml` it is the sibling repo, and their crates.io version is not the
same as their `main` (see *External Crate Dependencies* in `CLAUDE.md`). Quote the mechanism,
not the conclusion, so a reviewer can check the same lines instead of re-deriving your
reasoning. For a web-platform claim, read the spec text rather than describing it — and if
the guarantee is weaker than you want, write the weaker one (see
[no-guarantee-stronger-than-code-enforces.md](../comments-documentation/no-guarantee-stronger-than-code-enforces.md)).

```rust
// WRONG — the whole fix is this sentence, and the sentence is a guess.
// "Collapsing Some(Ok(_)) into a unit `Ready` variant stops the remount,
//  because Memo is cached."

// RIGHT — the mechanism, at the version that shipped, so it is checkable.
// reactive_graph-0.2.14: `ArcMemo::new`'s doc + `ReadUntracked::read_untracked`
// (which calls `update_if_necessary()`) — a Memo recomputes lazily on read but
// only marks subscribers dirty when the new value is `PartialEq`-unequal to the
// old one. Collapsing `Some(Ok(_))` to a unit `Ready` is therefore what makes
// the refetch stop propagating; it is load-bearing, not decorative.
```

Five reviews across the `2026-08-20` → `2026-08-24` logs turned on exactly this move, and
all three crate versions they name still match `Cargo.lock` today:

- **KYO-429 (2026-08-22, `09:15` cycle 3)** — the `Memo` notify-on-change question above,
  settled against `reactive_graph-0.2.14`. The reviewer's phrasing is the point: the fix is
  *"real, not a no-op"* — a verdict only the library's source could produce.
- **KYO-443 + KYO-426 (2026-08-22, `08:40`)** — the fix depended on Leptos re-subscribing
  conditionally. Confirmed in `reactive_graph-0.2.14`'s `src/computed/inner.rs`:
  `any_subscriber.clear_sources(...)` runs before every recompute, so an untaken branch
  genuinely does not resubscribe. The companion `PartialEq` dedupe claim was checked the same
  way (`Memo::new` requires `T: PartialEq`; the inner impl only calls `sub.mark_dirty()` when
  `changed`).
- **KYO-440 (2026-08-24, `13:49` cycle 2)** — whether a superseded popup monitor could
  double-fire came down to `gloo-timers 0.3.0`'s `impl Drop for Interval`/`Timeout` calling
  `clear_interval`/`clear_timeout` synchronously and unconditionally. Read, not assumed; with
  it the race is *"structurally impossible"* rather than merely unlikely.
- **KYO-401 (2026-08-21, `14:20`)** — `axum-core 0.5.6`: `impl IntoResponse for String` and
  for `&'static str` both route through `Cow<'static, str>::into_response()`, so a new
  owned-`String` body is byte-identical to the `&'static str` it replaced. The whole
  "is this a behaviour change?" question, answered by one file.
- **KYO-390 (2026-08-20, `18:50`)** — a lockfile-only PR, where the risk argument was that
  the moved `windows-sys` edges are all `cfg(windows)`-gated. The reviewer pulled the real
  `Cargo.toml` for `errno`, `home`, `dirs-sys` and `dlmalloc` out of the local registry cache
  and confirmed each one's `[target."cfg(windows)".dependencies]` declaration, rather than
  inferring the gating from the crate names.

Sibling of
[verify-lint-fixes-on-the-toolchain-that-produces-them.md](verify-lint-fixes-on-the-toolchain-that-produces-them.md):
that rule is about the *compiler* you ran not matching the one CI runs; this one is about the
*libraries* you reasoned about not matching the ones you linked.
