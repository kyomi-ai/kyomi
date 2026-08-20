# Use `action.pending()` as the double-dispatch guard

When the original `spawn_local` code used a manual `is_toggling` or `is_loading` signal to prevent concurrent dispatches, the `Action` equivalent is `action.pending().get_untracked()` in the dispatch callback — not dropping the guard entirely.

```rust
// WRONG — drops the concurrency guard when converting to Action
let toggle_action = Action::new(move |id: &String| { /* ... */ });
let handle_toggle = move |_| {
    toggle_action.dispatch(id.clone());  // no guard — double-click races
};

// RIGHT — Action's built-in pending() replaces the manual guard
let toggle_action = Action::new(move |id: &String| { /* ... */ });
let handle_toggle = move |_| {
    if !toggle_action.pending().get_untracked() {
        toggle_action.dispatch(id.clone());
    }
};
```
