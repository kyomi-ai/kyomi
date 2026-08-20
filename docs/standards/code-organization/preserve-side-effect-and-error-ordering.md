# Preserve side-effect and error ordering when refactoring

When extracting a helper or reordering validation in a "no behavior change" refactor, the *observable* behavior includes more than HTTP status and body — it includes **which side effects run** and **in what order errors are produced**. Moving a fallible or side-effecting step earlier changes behavior for the combined-failure input (e.g. bad datasource slug AND missing encryption key now surfaces the encryption error first), and moving a side-effecting call (network, DB write) ahead of an early-return guard causes it to run on paths that previously short-circuited.

**Rule:** When you relocate a validation, guard, or context-building step, trace every input class that could fail at more than one point and confirm the *first* error and the *set of side effects* are unchanged. If a helper needs a value that is expensive or side-effecting to compute, pass it lazily (a closure invoked past the resolve/guard step) rather than eagerly at the call site.

```rust
// WRONG — build_user_context() does a network token refresh + DB write;
// running it before resolve means an invalid slug now triggers that side effect
let user_context = build_user_context(&state, &user).await?;
let ds = resolve_datasource(&db, &slug).await?; // was first before the refactor

// RIGHT — resolve/guard first, build the side-effecting context only after
let ds = resolve_datasource(&db, &slug).await?;
let user_context = build_user_context(&state, &user).await?;
```

Flagged repeatedly in KYO-195/196 reviews (behavior-change-in-refactor): eager `user_context` construction and an encryption-key check both moved ahead of `resolve_datasource`, changing side effects and first-error for invalid-slug inputs.
