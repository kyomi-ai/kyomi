# Never mix signal lifetimes in `Signal::derive` without `try_get()`

**Enforcement: advisory — this does NOT fail the build.** `scripts/lint/check-disposal-safety.sh` Rule B prints a `WARN:B` line for a bare `.get()` inside `Signal::derive` / `Memo::new`, but `WARN`-tagged findings are excluded from the script's failure path, so CI exits 0 regardless. Rule B cannot tell a genuinely mixed-lifetime derive from a same-scope one, so it is intentionally non-blocking. Treat it as a prompt to check the derive yourself, not as a guarantee. See *Enforcement status* above.

A `Signal::derive` that subscribes to BOTH a long-lived signal (Layout-scoped, e.g. `SyncStore` data) AND a page-scoped signal (e.g. search/sort/filter) creates a disposal race. When the user navigates away, the page-scoped signals are disposed — but the Layout-scoped signal can still trigger re-evaluation of the derive (e.g. via a WebSocket sync update), causing it to call `.get()` on the disposed page signals → panic.

**Rule:** If a `Signal::derive` reads from signals with different lifetimes, use `.try_get()` for the shorter-lived ones. Return a sensible default (empty vec, default sort, etc.) if they're disposed.

```rust
// WRONG — derive subscribes to both sync_store (Layout) and query (page-scoped)
let filtered = Signal::derive(move || {
    let items = sync_store.all_items().get();    // Layout-scoped, lives forever
    let q = search_query.get();                  // 💥 page-scoped, may be disposed
    filter(items, q)
});

// RIGHT — try_get() for page-scoped signals, graceful fallback
let filtered = Signal::derive(move || {
    let items = sync_store.all_items().get();
    let q = search_query.try_get().flatten();    // None if disposed
    match q {
        Some(ref query) => filter(items, query),
        None => items,                           // unfiltered fallback
    }
});
```

**This is the root cause of most "reactive value already disposed" panics.** The previous 12+ tickets for this panic class were fixed one-by-one; this pattern prevents the entire class.
