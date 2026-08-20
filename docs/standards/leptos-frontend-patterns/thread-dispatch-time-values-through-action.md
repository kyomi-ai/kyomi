# Thread dispatch-time values through the Action return type

When converting `spawn_local` to `Action`, the `spawn_local` pattern captures values by closure at dispatch time. But the `Effect` watching `action.value()` fires at effect-evaluation time — if it reads live signals instead of values returned from the action, it can see post-edit values that the server never received.

**Rule:** Any value that was captured at dispatch time in the original `spawn_local` must either be part of the Action input or returned through the Action result. Never read live signals in the Effect for values that should reflect the dispatch-time state.

```rust
// WRONG — Effect reads live signals that may have changed during the async call
let save_action = Action::new(move |_: &()| {
    let title = title_signal.get_untracked();
    async move { save_to_server(title).await }
});
Effect::new(move |_| {
    if let Some(Ok(())) = save_action.value().get() {
        set_saved_title.set(title_signal.get());  // 💥 reads CURRENT value, not saved value
    }
});

// RIGHT — dispatch-time values returned through the action result
let save_action = Action::new(move |_: &()| {
    let title = title_signal.get_untracked();
    async move {
        save_to_server(&title).await?;
        Ok(title)  // return the value that was actually saved
    }
});
Effect::new(move |_| {
    if let Some(Ok(saved_title)) = save_action.value().get() {
        set_saved_title.set(saved_title.clone());  // uses the actual saved value
    }
});
```
