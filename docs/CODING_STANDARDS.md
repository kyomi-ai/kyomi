# Coding Standards

This document captures coding standards learned from code reviews. It evolves over time — the orchestrator updates it at the start of each `/agent-driven-development` session by mining recent review logs for recurring patterns.

**Read this document before implementing any feature.** Every rule here exists because agents have repeatedly made the same mistake, and a code reviewer had to catch it.

Rules in this document are specific to patterns observed in this codebase. For general architecture principles, see `CLAUDE.md`. For the full anti-pattern checklist used by reviewers, see `.claude/agents/code-review-architect.md`.

---

## Error Handling

*Standards for how errors should be propagated, contextualized, and reported.*

*(No entries yet — this section will be populated as review patterns emerge.)*

## Leptos / Frontend Patterns

*Standards specific to Leptos components, reactivity, SSR/hydration, and frontend architecture.*

### Use `.try_set()` / `.try_update()` in deferred execution contexts

Signal writes inside `spawn_local`, `Closure::new`, `set_timeout`, or any callback that outlives the reactive scope must use `.try_set()` / `.try_update()` instead of `.set()` / `.update()`. The user may navigate away before the callback fires, disposing the signal — `.set()` panics, `.try_set()` silently returns `false`.

**Rule:** Synchronous writes *before* a `spawn_local` or in `Effect::new` blocks are fine with `.set()` — the signal is guaranteed to be alive. Only deferred writes (inside the async block, inside a `.forget()`-ed Closure, inside a Timeout callback) need the `try_` variant.

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

### WASM-only `#[cfg]` blocks must compile on the WASM target

Variables used inside `#[cfg(target_arch = "wasm32")]` blocks must be parameters of the enclosing function (or otherwise available in scope on WASM). `cargo check` on the native target silently skips the block body, so missing variables won't be caught until the WASM build.

**Rule:** After modifying any function that contains `#[cfg(target_arch = "wasm32")]`, verify with:
```bash
cargo check --target wasm32-unknown-unknown -p kyomi-ui --features hydrate
```

```rust
// WRONG — compiles on native, fails on WASM (datasource_type not in scope)
fn run_arrow_query(slug: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let dt = datasource_type.clone(); // E0425 on WASM
    }
}

// RIGHT — parameter is available on both targets
fn run_arrow_query(slug: &str, datasource_type: String) {
    #[cfg(target_arch = "wasm32")]
    {
        let dt = datasource_type.clone(); // works
    }
}
```

### Never snapshot a `Signal` prop into a local variable — derive from it

Calling `signal.get()` on a prop and storing the result in a local `String` (or any non-reactive variable) creates a frozen snapshot. Wrapping that snapshot in a new `Signal::stored` or `create_signal` doesn't restore reactivity — it just adds a layer of indirection over dead data. Use `Signal::derive(move || original_signal.get())` to keep the derived value reactive.

**Rule:** If a component receives a `Signal<T>` prop and needs to pass a derived value to a child, use `Signal::derive` closing over the original signal — never `.get()` into a local and re-wrap.

```rust
// WRONG — captures a snapshot, child never sees updates
let val = signal_prop.get();            // snapshot at mount time
let val_sig = Signal::stored(val);      // wraps the dead snapshot
// child sees the initial value forever

// RIGHT — derive keeps reactivity alive
let derived = Signal::derive(move || signal_prop.get());
// child re-renders when signal_prop changes
```

### Never read signals eagerly inside `ChildrenFn` / `Arc<dyn Fn() -> AnyView>` closures that share scope with inputs

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

## API & Server Functions

*Standards for server functions, REST endpoints, and the boundary between frontend and backend.*

*(No entries yet.)*

## Data & State Management

*Standards for database access, caching, state synchronization, and data flow.*

*(No entries yet.)*

## Security

*Standards for encryption, authentication, credential handling, and input validation.*

*(No entries yet.)*

## Code Organization

*Standards for module structure, imports, shared utilities, and avoiding duplication.*

### Use `OnceLock<Regex>` for repeated regex patterns

If a `Regex::new(...)` pattern appears more than once in the codebase (or is called in a hot path), extract it into a `static OnceLock<Regex>`. Compiling the same regex repeatedly wastes CPU and invites copy-paste drift when the pattern needs updating.

**Rule:** Before writing `Regex::new(...)` inline, grep for the pattern string. If it already exists elsewhere, extract both into a shared static. New regex patterns that will be called more than once should start as statics.

```rust
use std::sync::OnceLock;
use regex::Regex;

// WRONG — same pattern compiled in 4 different call sites
fn find_chartml_block(content: &str) -> Option<&str> {
    let re = Regex::new(r"(?s)```chartml\s*\n(.*?)```").unwrap();
    re.captures(content).map(|c| c.get(1).unwrap().as_str())
}

// RIGHT — compiled once, shared across all call sites
fn chartml_fence_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)```chartml\s*\n(.*?)```").unwrap())
}

fn find_chartml_block(content: &str) -> Option<&str> {
    chartml_fence_regex().captures(content).map(|c| c.get(1).unwrap().as_str())
}
```

## Testing

*Standards for test structure, assertions, and what must be tested.*

*(No entries yet.)*
