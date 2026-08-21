# Use `.try_set()` / `.try_update()` in ALL deferred execution contexts

**Enforcement: blocking.** `scripts/lint/check-disposal-safety.sh` Rule A catches bare `.set()` / `.update()` inside `spawn_local` and other deferred callbacks, and **fails CI**. This is the only pattern in this section that stops a merge. Escape hatch, requiring a ≥5-character justification: `// lint-allow: disposal-safe=<why>`. See *Enforcement status* above.

Signal writes inside `spawn_local`, `spawn_scoped`, `Closure::new`, `set_timeout`, or any callback that outlives the reactive scope must use `.try_set()` / `.try_update()` instead of `.set()` / `.update()`. The same applies to `Callback::run()` — use `Callback::try_run()` when the callback is invoked inside an Action's async block or any deferred context, because the component that created the callback may have been unmounted. The user may navigate away before the callback fires, disposing the signal or stored value — `.set()` / `Callback::run()` panics, `.try_set()` / `Callback::try_run()` silently returns `false` / `None`.

**Rule:** Synchronous writes *before* a `spawn_local` or in `Effect::new` blocks are fine with `.set()` — the signal is guaranteed to be alive. Only deferred writes (inside the async block, inside a `.forget()`-ed Closure, inside a Timeout callback) need the `try_` variant. This is a belt-and-suspenders defense — the primary fix is to use `Action` or `spawn_scoped`, but `try_` methods catch any remaining edge cases.

```rust
// WRONG — panics if user navigates away before the fetch completes
spawn_local(async move {
    let result = fetch_data().await;
    loading.set(false);        // 💥 signal may be disposed
    data.update(|d| *d = result);
});

// RIGHT — deferred writes use try_ variants
spawn_local(async move {
    let result = fetch_data().await;
    loading.try_set(false);    // returns false if disposed, no panic
    data.try_update(|d| *d = result);
});

// ALSO RIGHT — synchronous write before spawn_local is safe
loading.set(true);             // signal is alive here, .set() is fine
spawn_local(async move { /* ... */ });
```
