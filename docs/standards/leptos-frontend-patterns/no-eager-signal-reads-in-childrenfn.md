# Never read signals eagerly inside `ChildrenFn` / `Arc<dyn Fn() -> AnyView>` closures that share scope with inputs

**Enforcement: review-only.** No lint catches this — it requires knowing that a `.get()` sits inside a `ChildrenFn` body. See *Enforcement status* above.

If a `ChildrenFn` closure (used by `Modal` footer, `Transition` fallback, etc.) reads a signal with `.get()`, every signal change re-executes the entire closure and rebuilds its DOM. If the Modal/component re-renders children alongside the footer, this destroys any `<input>` elements in the body — causing focus loss on every keystroke.

**Rule:** Never call `.get()` on a signal directly inside a `ChildrenFn` closure if that signal is also written by an `<input>` in the component's body. Instead, use `Signal::derive` to create fine-grained derived signals that only update the specific prop (e.g. `disabled`) without rebuilding the DOM.

```rust
// WRONG — reads transfer_confirmation.get() in the footer closure.
// Every keystroke in the input rebuilds the footer, which triggers
// the Modal to re-call children(), destroying the input.
let footer: Arc<dyn Fn() -> AnyView> = Arc::new(move || {
    let conf = transfer_confirmation.get(); // ← causes full rebuild
    let disabled = conf != workspace_name.get();
    view! { <Button disabled=disabled>"Submit"</Button> }.into_any()
});

// RIGHT — Signal::derive creates a fine-grained subscription.
// Only the button's `disabled` attribute updates, no DOM rebuild.
let footer: Arc<dyn Fn() -> AnyView> = Arc::new(move || {
    let disabled = Signal::derive(move || {
        transfer_confirmation.get() != workspace_name.get()
    });
    view! { <Button disabled=disabled>"Submit"</Button> }.into_any()
});
```

**General principle:** Reactive closures that rebuild DOM (`{move || ...}` in views, `ChildrenFn`, `Arc<dyn Fn() -> AnyView>`) must never contain `<input>` elements AND read the signal that input writes. Either:
1. Move the input outside the reactive closure (into static view structure)
2. Use `Signal::derive` so the signal subscription is scoped to a leaf attribute, not the whole closure
