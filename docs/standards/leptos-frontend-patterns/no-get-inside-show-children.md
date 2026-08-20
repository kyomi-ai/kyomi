# Never call `.get()` on signals inside `<Show>` children — pass Signals as props

**Enforcement: review-only.** No lint catches this — `.get()` in a `<Show>`'s `when=` prop is correct while the same call in its children is a bug, and only an AST can tell them apart. See *Enforcement status* above.

In Leptos 0.7, `<Show>` wraps children in a reactive closure: `move || if when() { children() } else { fallback }`. Any `.get()` call inside `children()` subscribes the **parent reactive scope** to that signal. When the signal changes, the parent scope re-runs, calling `children()` again — destroying and recreating all child components, even if the `when` condition hasn't changed. This causes visible DOM flashing and destroys internal component state (timers, scroll position, expanded/collapsed state).

**Rule:** Never call `.get()` on signals inside a `<Show>` children block. Instead, create `Signal::derive` closures *outside* the `<Show>` and pass them as `Signal<T>` props to child components. If the child component only accepts plain values (not Signals), refactor it to accept `Signal<T>` props so it can be created once and reactively update.

```rust
// WRONG — .get() inside <Show> children subscribes parent scope to thinking_state.
// Every thinking_state change destroys and recreates AgentThinking.
<Show when=should_show_thinking>
    {
        let events = thinking_events_fn();  // calls thinking_state.get()
        let active = is_active_fn();        // calls thinking_state.get()
        view! { <AgentThinking thinking_events=events is_active=active /> }
    }
</Show>

// RIGHT — Signal::derive created outside <Show>, passed as Signal props.
// AgentThinking is created once and reactively updates without remounting.
let events_signal = Signal::derive(thinking_events_fn);
let active_signal = Signal::derive(is_active_fn);
<Show when=should_show_thinking>
    <AgentThinking thinking_events=events_signal is_active=active_signal />
</Show>
```

Flagged in KYO-38 rework — the `AgentThinking` component flashed on every tool call because `thinking_state.get()` inside `<Show>` children caused full component re-creation on each thinking event.
