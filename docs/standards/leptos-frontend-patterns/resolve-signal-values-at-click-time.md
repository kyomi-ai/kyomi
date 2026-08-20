# Resolve derived signal values at click time, not render time

When a reactive closure (`{move || ...}`) builds a button whose `on:click` opens a popup or navigates to a URL derived from signals, the URL must be resolved inside the click handler — not captured into the closure's scope at render time. The outer closure re-runs when its tracked signals change, but intermediate signal values (like a `slug`-derived URL) may update independently without re-triggering the closure.

**Rule:** Use `signal.get_untracked()` inside `on:click` handlers for values that should reflect the current state at interaction time.

```rust
// WRONG — URL captured at render time, stale if slug changes
let connect_url_val = connect_url.get(); // captured when closure runs
view! {
    <button on:click=move |_| {
        open_oauth_popup(&connect_url_val); // uses stale value
    }>"Connect"</button>
}

// RIGHT — URL resolved at click time
view! {
    <button on:click=move |_| {
        let url = connect_url.get_untracked(); // fresh value at click time
        open_oauth_popup(&url);
    }>"Connect"</button>
}
```
