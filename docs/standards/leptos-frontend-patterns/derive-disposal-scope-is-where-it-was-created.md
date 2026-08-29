# A derive's disposal scope is where it was CREATED, not what it reads

**Enforcement: advisory.** Same lint as its neighbour
[`no-mixed-signal-lifetimes-in-derive.md`](./no-mixed-signal-lifetimes-in-derive.md) —
`scripts/lint/check-disposal-safety.sh` Rule B, `WARN:B` only, does not fail CI. This
document does not change enforcement; it corrects the reasoning a `// lint-allow:
disposal-safe=<why>` justification is allowed to rely on. See *Enforcement status*
above before treating either document as a gate.

## The mistake

KYO-500 (PR #428) added 55 `lint-allow` justifications for bare `.get()` inside
`Signal::derive`/`Memo::new`. Twelve of them read:

> "single-source derive, Layout-scoped `<X>` only — no page-scoped signal mixed in, so
> no disposal hazard"

The reasoning: if a derive reads *only* a Layout-scoped signal (something that outlives
the page, e.g. `user_ctx_resource`, a `WebSocketContext`, a `SyncStore`), there is no
page-scoped signal inside it to panic on when the page disposes.

**That reasoning checks the wrong thing.** It asks "what does this derive read?" The
question that actually decides disposal safety is "where was this derive *itself*
created, and does anything read it after that scope disposes?"

## The mechanism (reactive_graph 0.2.14, pinned via `Cargo.lock`; leptos 0.8.20)

Source at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/reactive_graph-0.2.14`.

1. **`Signal::derive` (and `Memo::new`) are themselves arena items — not bare
   closures.** `src/wrappers.rs:631-649`:
   ```rust
   pub fn derive(derived_signal: impl Fn() -> T + Send + Sync + 'static) -> Self {
       // ...
       Self {
           inner: ArenaItem::new_with_storage(SignalTypes::DerivedSignal(Arc::new(derived_signal))),
           // ...
       }
   }
   ```
   `Memo<T>` does the same — `src/computed/memo.rs:180`:
   `inner: ArenaItem::new_with_storage(ArcMemo::new(fun))`.

2. **The arena item registers against whatever `Owner` is current at CONSTRUCTION
   time.** `src/owner/arena_item.rs:47-64`:
   ```rust
   pub fn new_with_storage(value: T) -> Self {
       let node = Arena::with_mut(|arena| arena.insert(/* ... */));
       OWNER.with(|o| {
           if let Some(owner) = o.borrow().as_ref().and_then(|o| o.upgrade()) {
               owner.register(node);   // owner.rs:329 -> nodes.push(node)
           }
       });
       Self { node, ty: PhantomData }
   }
   ```
   Nothing here inspects what the closure passed to `Signal::derive` reads. A derive
   created while a page component's `Owner` is current is registered on that page
   `Owner`, full stop — regardless of whether the closure body reads a Layout-scoped
   resource, a page-scoped signal, or a constant.

3. **Disposal removes the registering owner's nodes from the arena.**
   `src/owner.rs:519` (`impl Drop for OwnerInner`) and `src/owner.rs:554`
   (`impl Cleanup for RwLock<OwnerInner>`) both: clean up child scopes first, *then*
   `for node in nodes { _ = arena.remove(node); }` on this owner's own nodes.

4. **A bare `.get()` on a removed node panics.** `src/traits.rs:393`:
   `fn get(&self) -> Self::Value { self.try_get().unwrap_or_else(unwrap_signal!(self)) }`,
   and `unwrap_signal!` (`src/traits.rs:68`) panics with "Tried to access a reactive
   value that has already been disposed." The read path is
   `ArenaItem::try_with_value` -> `S::try_with(self.node, fun)`
   (`owner/arena_item.rs:103-105`), and disposal state itself is
   `IsDisposed::is_disposed` (`owner/arena_item.rs:126-129`):
   `Arena::with(|arena| !arena.contains_key(self.node))`.

**Conclusion:** a page-created `Signal::derive`/`Memo::new` is page-owned. "It reads
only a Layout-scoped signal" tells you the *inner* read (of the Layout-scoped resource)
is safe — that resource outlives the page. It says nothing about the *outer* read (of
the derive itself), which is governed by the derive's own Owner: the page.

## What actually makes such a derive safe

Not what it reads. Whether **every reader of the derive is disposed together with the
derive** — i.e., every read site is a descendant reactive scope of the same page `Owner`
(a `view!` closure, a nested `<For>` child, an `Effect::new` created in the same
component body, a synchronous event handler triggered while the page is mounted). Point
3 above guarantees children are cleaned up *before* the owning scope's own nodes are
removed, so if every reader lives in a child scope of the page, no reader can observe
the derive after it is gone — the readers stop running first.

This becomes unsafe the moment a reader can outlive the page: a value captured into a
`spawn_local` future that reads the derive *after* an `.await` (rather than capturing a
plain value beforehand), a handle stored in a longer-lived context or a global registry,
or a callback registered with something Layout-scoped (e.g. a WebSocket dispatcher) that
keeps invoking it after the page unmounts.

```rust
// WRONG justification — asks what the derive reads, not who reads the derive.
// Single-source derive (Layout-scoped user_ctx_resource only) — no page-scoped
// signal mixed in, so no disposal hazard.
let multi_user_enabled = Signal::derive(move || {
    user_ctx_resource.get() // <- this call is what panics; user_ctx_resource
        .and_then(|r| r.ok())                     // outliving the page does not
        .and_then(|ctx| ctx.capabilities.get("multi_user_enabled").copied())
        .unwrap_or(false)
});

// RIGHT justification — the derive is page-owned; the check is on its readers.
// Page-owned derive: created in this page component body, so its own Owner is
// this page. Safe because every read of it (chat_list.rs:774, 889) is inside
// this page's own view tree -- a descendant scope of the same page Owner --
// so the derive and its readers are disposed together (KYO-548).
let multi_user_enabled = Signal::derive(move || {
    user_ctx_resource
        .get() // lint-allow: disposal-safe=page-owned derive, all reads confined to this page's own view/effect tree (KYO-548)
        .and_then(|r| r.ok())
        .and_then(|ctx| ctx.capabilities.get("multi_user_enabled").copied())
        .unwrap_or(false)
});
```

If you cannot enumerate every reader, or a reader genuinely can outlive the derive
(a detached `spawn_local` reading it post-`.await`, a Layout-scoped consumer), do not
write a same-scope justification — convert to `.try_get()` with a stated fallback
instead, per
[`no-mixed-signal-lifetimes-in-derive.md`](./no-mixed-signal-lifetimes-in-derive.md).

## Live reproduction status

No panic was reproduced through real routing/effect machinery for the 12 sites this
finding applies to — every reader was verified (by tracing call sites, not by running a
browser) to be a descendant reactive scope of the same page component, so the
mechanism above does not fire for any of them today. This flow does not build WASM or
run a browser (see `~/repos/kyomi-private/docs/BUILD_AND_TESTING.md`); a synthetic
`reactive_graph` `Owner`-hierarchy test proving the *mechanism* (child derive disposed,
parent-held signal it reads still alive) lives in
`crates/kyomi-ui/src/pages/chat/chat_page.rs` test module — see the module doc comment
there for what it does and does not prove. That synthetic result is not a substitute for
a live reproduction; none was attempted beyond tracing call sites, because no path was
found where a reader of these 12 derives could plausibly outlive the page.

## Precedent

- **KYO-500** (PR #428) — introduced the 55 `lint-allow` justifications, 12 of which
  used the "Layout-scoped X only" form this document corrects.
- **KYO-548** — raised the heuristic gap during KYO-500's review, and is the ticket
  this document and the 12 corrected justifications were produced under.
- `docs/standards/leptos-frontend-patterns/no-mixed-signal-lifetimes-in-derive.md` — the
  mixed-lifetime case; this document is about the case where nothing is mixed and the
  heuristic still fails.
