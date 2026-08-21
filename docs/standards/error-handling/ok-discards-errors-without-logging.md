# Never use `.ok()` or `.ok()?` to discard errors without logging

Converting a `Result` to `Option` via `.ok()` silently swallows the error — no log, no trace, no signal that something went wrong. In production, this turns debugging into archaeology: the symptom appears far from the cause, and there's no record of what failed.

**Rule:** Before calling `.ok()`, add a `.map_err()` that logs the error at `warn!` or `debug!` level. If the error is truly unactionable (best-effort fire-and-forget), at minimum log at `debug!` so it shows up in verbose traces.

```rust
// WRONG — error silently discarded, no trace
let cost = fetch_openrouter_cost(&client, &gen_id).await.ok();
let settings = load_title_model(&pool, workspace_id).await.ok().flatten();

// RIGHT — error logged before discarding
let cost = fetch_openrouter_cost(&client, &gen_id)
    .await
    .map_err(|e| debug!(generation_id = %gen_id, error = %e, "cost fetch failed"))
    .ok();

let settings = load_title_model(&pool, workspace_id)
    .await
    .map_err(|e| warn!(%workspace_id, error = %e, "failed to load title model"))
    .ok()
    .flatten();
```

Flagged across 3 reviews in May 2026: KYO-37 (`serde_json::Error` discarded), KYO-36 (network errors discarded), KYO-34 (`WorkspaceAiConfigError` discarded).
