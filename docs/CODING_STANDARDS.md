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

*(No entries yet.)*

## Testing

*Standards for test structure, assertions, and what must be tested.*

*(No entries yet.)*
