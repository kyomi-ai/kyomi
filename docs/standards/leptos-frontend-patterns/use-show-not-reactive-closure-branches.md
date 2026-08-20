# Use `<Show>` for conditional component rendering, not reactive closure branches

**Enforcement: review-only.** No lint catches this — it requires knowing that the component inside a reactive branch owns a reactive scope. See *Enforcement status* above.

When a reactive closure (`{move || { ... }.into_any()}`) conditionally renders a component that owns its own reactive scope (e.g., `DynSelect` → `Popover` → `Effect::new`), the branch switch destroys and recreates the component's internal signals. If the component's `Effect` fires during disposal, it accesses dead signals → panic. The Leptos `<Show>` component handles component lifecycle correctly — it mounts/unmounts through the framework's ownership tree.

**Rule:** Never gate a component with internal reactive state (popover, modal, effect-owning widget) inside a `{move || ...}` view closure. Use `<Show when=condition>` with a `fallback` instead.

```rust
// WRONG — reactive closure branch creates/destroys DynSelect (which owns Popover/Effect)
view! {
    {move || {
        if has_projects.get() {
            view! { <DynSelect options=projects /> }.into_any()
        } else {
            view! { <input type="text" /> }.into_any()
        }
    }}
}

// RIGHT — <Show> manages component lifecycle safely
view! {
    <Show when=move || has_projects.get()
          fallback=move || view! { <input type="text" /> }>
        <DynSelect options=projects />
    </Show>
}
```

Flagged in KYO-14 review — four `DynSelect` instances gated by reactive closures caused disposal panic risk on OAuth connect.
