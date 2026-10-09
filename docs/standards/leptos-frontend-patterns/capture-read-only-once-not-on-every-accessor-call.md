# Capture a `ReadSignal` once at construction — re-deriving it via `.read_only()` on every accessor call panics after disposal

`RwSignal::read_only()` is not a cheap view into an existing handle. Each call allocates a
*fresh* `ArenaItem`, registered against whatever `Owner` is current at the moment it runs — and
if the underlying `RwSignal` is already disposed, the call itself panics, before any `.get()` or
`.try_get()` on the result even happens. A struct whose accessor method does
`self.some_signal.read_only()` inline looks harmless: it type-checks, it works every time the
struct is used while its owning scope is alive, and nothing about the code suggests the call
itself — not the subsequent read — is the unsafe part. The panic only shows up once a caller
holds the *struct* past the point its owning reactive scope disposes and then invokes the
accessor, which is exactly the shape of a chat/session object handed to a WebSocket callback or
an async continuation.

Verified against the pinned dependency source, not inferred: `RwSignal::read_only()`
(`reactive_graph-0.2.14`, `src/signal/rw.rs:173-184`) panics via
`unwrap_or_else(unwrap_signal!(self))` if `self` is already disposed. This is a different failure
point from the read methods this codebase already has a rule for — `ReadSignal::try_get()` /
`try_get_untracked()` are non-panicking on a disposed *target*, but that safety only helps once
you already hold a `ReadSignal`. Calling `.read_only()` again to get one is itself the unsafe
step, and there is no `try_read_only()` to swap in.

**Rule:** Call `.read_only()` (or any other arena-allocating "derive a handle" method) exactly
once, at construction time, while the owning `Owner` is still current — typically in the type's
`new()`. Store the returned `ReadSignal` as a field and have every accessor return the stored
`Copy` handle, or call `.try_get_untracked()` on that stored handle. Never call `.read_only()`
(or `.write_only()`) again later from inside a method that might run post-disposal.

```rust
// WRONG — every call to `messages()` re-derives a fresh ReadSignal by calling
// `.read_only()` on the underlying RwSignal. If `messages` is invoked after the
// owning scope (e.g. the chat page) has disposed, `read_only()` itself panics —
// "Tried to access a reactive value that has already been disposed" — even
// though nothing here ever calls a non-`try_` get.
struct ChatEngine {
    messages: RwSignal<Vec<Message>>,
}

impl ChatEngine {
    pub fn messages(&self) -> ReadSignal<Vec<Message>> {
        self.messages.read_only()
    }
}

// RIGHT — the ReadSignal is captured once, in the constructor, while the
// owning Owner is current. The accessor returns the stored Copy handle;
// callers that need disposal-safety use try_get_untracked() on it, which
// safely returns None instead of panicking.
struct ChatEngine {
    messages: RwSignal<Vec<Message>>,
    messages_read: ReadSignal<Vec<Message>>,
}

impl ChatEngine {
    pub fn new(messages: RwSignal<Vec<Message>>) -> Self {
        Self {
            messages,
            messages_read: messages.read_only(), // captured once, here
        }
    }

    pub fn messages(&self) -> ReadSignal<Vec<Message>> {
        self.messages_read
    }
}
```

Flagged and fixed in **KYO-781** (review log `2026-09-24`, `11:20` — "KYO-781 chat_engine
disposal-safety fix"): `ChatEngine`, `ChatStateMachine`, and `ThinkingManager`
(`crates/kyomi-ui/src/components/chat/{chat_engine,chat_state,thinking}.rs`) all had this exact
shape — a `ReadSignal` re-derived via `.read_only()` inside an accessor method rather than
captured once. The reviewer reproduced the panic directly: reverting `ChatEngine::messages()`
back to `self.messages.read_only()` and running the disposal test reproduced the exact panic at
`rw.rs:181:37`, then confirmed the fix (capture-once) made the same test pass by returning `None`
via `try_get_untracked()` on the stored handle instead. Re-reviewed and re-signed after a rebase
(review log `2026-09-24`, `10:30`), where the diff was confirmed byte-identical to the originally
signed version and the same source-level mechanism was independently re-verified.

Distinct from [derive-disposal-scope-is-where-it-was-created.md](derive-disposal-scope-is-where-it-was-created.md):
that rule is about `Signal::derive`/`Memo::new`, whose own arena registration follows the `Owner`
current when the *derive itself* is constructed — the fix there is about who is allowed to read
the derive, not about when the derive is created. Here, nothing is wrong with the read; the
problem is a second, later call to `.read_only()` acting as if it were free. Distinct from
[try-set-try-update-in-deferred-contexts.md](try-set-try-update-in-deferred-contexts.md): that
rule swaps a panicking write method for its `try_` sibling; `.read_only()` has no `try_` sibling
to swap to; the only fix is to stop calling it more than once.
