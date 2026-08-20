# Never use raw `spawn_local` for user-triggered mutations — use `Action`

**Enforcement: review-only.** No lint catches this — distinguishing a user-triggered mutation from a sanctioned `spawn_local` needs AST awareness. See *Enforcement status* above.

**This is the #1 source of WASM panics in the codebase.** Raw `spawn_local` spawns a detached future that outlives the component scope. When the user navigates away mid-async, the future completes and accesses disposed signals → panic at `reactive_graph/src/traits.rs:361`.

Leptos provides `Action` as the framework primitive for "user clicks → async call → update UI." It manages pending state, result signals, and scope lifecycle automatically.

**Rule:** If the pattern is "user interaction triggers async server call, then update UI," use `Action`. Reserve `spawn_local` for cases where Action genuinely doesn't fit (WebSocket handlers, multi-step orchestrations, fire-and-forget with no UI update).

```rust
// WRONG — raw spawn_local for a user-triggered mutation
let (creating, set_creating) = signal(false);
let handle_create = move |_| {
    set_creating.set(true);
    spawn_local(async move {
        match create_thing().await {
            Ok(id) => navigate(&format!("/thing/{id}"));
            Err(e) => {
                toast_error(format!("Failed: {e}"));
                set_creating.set(false);  // 💥 panics if navigated away
            }
        }
    });
};

// RIGHT — Action handles lifecycle, pending state, and result
let create_action = Action::new(move |_: &()| async move {
    create_thing().await
});

// React to result in an Effect (dies with the component — no panic)
Effect::new(move |_| {
    if let Some(Ok(id)) = create_action.value().get() {
        navigate(&format!("/thing/{id}"));
    }
    if let Some(Err(e)) = create_action.value().get() {
        toast_error(format!("Failed: {e}"));
    }
});

// In view — pending state is built-in
view! {
    <Button on:click=move |_| create_action.dispatch(())
            disabled=Signal::derive(move || create_action.pending().get())>
        "Create"
    </Button>
}
```

**When Action doesn't fit:** Reserve `spawn_local` for cases where Action genuinely can't work:
- **`!Send` browser APIs** (`JsFuture`, `TimeoutFuture`, clipboard, `web_sys` DOM calls) — `Action::new` requires a `Send` future. Browser-only APIs are `!Send` on wasm32 and will cause compilation failures. Use `spawn_local` with `try_` signal writes for the deferred part.
- **WebSocket callbacks, long-lived subscriptions** — use `on_cleanup` to unsubscribe/teardown, and `try_set`/`try_get_untracked` for any signal access that might race with disposal.
- **Fire-and-forget with no UI update** (e.g., analytics pings, mark-as-read calls with no UI feedback).
