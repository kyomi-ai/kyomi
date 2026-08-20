# `Signal::derive` is not memoized — use `Memo` when the body does more than read

`Signal::derive` wraps a plain `Fn() -> T`: it re-runs its body on *every* read. So a reactive closure that reads a derive alongside an unrelated signal re-runs the derive's body whenever that unrelated signal changes, not just when the derive's own dependencies do. `Memo` caches its value and recomputes only when the signals it actually reads change. If the derived body is pure and cheap this is invisible — but if it logs, records a metric, or does real work, the re-runs are observable and wrong.

**Rule:** If a derived value's body has a side effect or is expensive, define it with `Memo::new` scoped to only the signals it reads, and have callers read the memo. Reserve `Signal::derive` for cheap, pure projections. Never place a `warn!`/`error!` inside a closure that a multi-resource reactive scope reads — the log stops meaning "this failed" and starts meaning "something nearby re-rendered."

```rust
// WRONG — the closure also reads `members`, so every members refetch re-runs
// current_user_id_from and re-emits its warn! for a failure that already happened
let is_owner = move || {
    let id = current_user_id_from(user_ctx.get()); // contains warn! on the Err path
    members.get().iter().any(|m| m.user_id == id && m.role == "owner")
};

// RIGHT — Memo scoped to user_ctx; the body (and its log) runs once per real change
let current_user_id = Memo::new(move |_| current_user_id_from(user_ctx.get()));
let is_owner = move || {
    let id = current_user_id.get();
    members.get().iter().any(|m| m.user_id == id && m.role == "owner")
};
```

Flagged as 🟡 in KYO-240 (2026-08-03): once the `UserContext` fetch failed, every later team action — remove member, role change, initiate/cancel transfer — bumped `members_version`/`transfers_version`, re-ran the enclosing closure, and re-logged "user context fetch failed", so one stale failure read as a stream of fresh ones. The identical shape one function away (`is_owner_from` re-awaiting a cached `Err`) was ticketed as KYO-304.
