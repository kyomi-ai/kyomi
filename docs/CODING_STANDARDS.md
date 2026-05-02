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
