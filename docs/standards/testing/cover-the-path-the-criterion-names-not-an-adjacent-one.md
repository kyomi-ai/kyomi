# Cover the path the acceptance criterion names, not the layer next to it

When a ticket says "partial write failure must still report `idle`" or "cover the changed
error paths with tests asserting the exact status code and body", it has named a *path*. A
test that exercises the pure helper the path calls, or the sibling handler in the same diff,
or the classifier feeding the guard clause, is coverage of something adjacent — and the
behaviour the ticket cares about stays unverified while the suite goes green and the test
count goes up.

This is close to invisible at author time, because the adjacent test is usually a genuinely
good test. It was written on purpose, it passes, it mutation-proves, and it sits in the diff
looking exactly like the AC being satisfied. The gap only shows up when someone asks the
narrower question: *if the production path regressed, would this specific test fail?* Twice in
this window the answer was no, and the demonstration was a one-line mutation that collapsed
the real guard and left every existing test passing.

The second half of this is the excuse that accompanies it. "There is no Postgres here", "no
write can ever succeed on this pool", "this only exists inside a reactive closure" — these
are claims about the environment, and they are checkable. Both times a reviewer checked, the
claim was wrong and a cheap in-process fixture existed: a branch that returns `Ok(())`
without touching pgvector, a new contract-test file, a small extraction of the decision into
a function the *real* path still calls. Note the shape of that last one: extracting a seam is
the right move only when the production path goes through the extraction. Extracting
something so it can be tested *instead of* the path is how you get here in the first place.

**Rule:** For each acceptance criterion, name the entry point it describes and assert
through that entry point. If the only coverage is one layer in — a pure helper, a sibling
call site, a classifier whose consumer holds the guard — say so explicitly rather than
marking the AC met. Before accepting "it can't be tested at that layer here", find the
cheapest branch of the real path that *can* succeed in this environment and build the fixture
there; if there is genuinely no seam, create one by extracting the decision into a function
the production path calls, then test through the production path anyway.

```rust
// WRONG — the AC is about the guard clause; the test covers the classifier.
// Collapsing `Explicit(projects) if !projects.is_empty()` and `Explicit(_)`
// into one unconditional arm still passes all ten of these.
#[test]
fn classify_returns_explicit_for_a_present_key() {
    assert_eq!(classify(&json!({"projects": ["a"]})), ConfiguredProjectScope::Explicit(vec!["a".into()]));
}

// RIGHT — the decision is extracted so the real path has a seam, and the
// test drives that seam the way the real path does. The same mutation now
// fails, with the filtered-to-empty case landing in Ok(Ok([])).
#[tokio::test]
async fn explicit_scope_that_filters_to_empty_is_recorded_as_a_skip() {
    let outcome = resolve_project_scope(&db, &ctx, &token).await;
    assert!(matches!(outcome, Err(_)), "an all-non-string projects list must not read as 'index everything'");
}
```

Three tickets in five days, all 🟡-blocking, all resolved in cycle 2:

- **KYO-385** (review log `2026-08-20`, `21:10` → `21:45`) — AC3 ("partial write failure ⇒
  still `idle`") was tested only against the extracted pure `resolve_run_outcome`
  (`crates/kyomi-auth/src/catalog/helpers.rs`), never through `index_catalog_sql`. The stated
  blocker — no table write can succeed on the test SQLite pool — was checked and was false:
  `cache_table`'s "schema unchanged AND embeddings exist" branch returns `Ok(())` and bumps
  `tables_indexed` without ever reaching pgvector, so a genuine mixed success/failure fixture
  was achievable in-process. Cycle 2 added
  `partial_write_failure_one_cached_one_failed_resolves_to_idle`
  (`crates/kyomi-agent/src/catalog/traits.rs`), which runs `index_catalog_sql` for real.
- **KYO-401** (review log `2026-08-21`, `14:20` → `13:15` cycle 2) — the AC asked for exact
  status code and body on the converted auth-critical handlers. `mcp.rs`'s three were covered
  by 21 pre-existing `contract_mcp.rs` tests; `oauth.rs`'s four (`oauth_authorize`,
  `oauth_authorize_continue`, `oauth_token`, `register_client`) had zero HTTP-boundary
  coverage before the diff and still had zero after it. The covered sibling half made the diff
  read as tested. Cycle 2 added `apps/server/tests/contract_oauth_client.rs` — 8 tests
  asserting exact status and whole body.
- **KYO-444** (review log `2026-08-22`, `08:10` → `08:55`) — `ConfiguredProjectScope::Explicit(_)`,
  the "non-empty key but every entry filters out" case the enum's own doc comment calls out,
  had no direct coverage; the guard clause consuming it was untested. Mutation-verified by the
  reviewer: replacing both `Explicit` arms with one unconditional pass-through still passed all
  ten existing tests — *"the original bug wearing a new hat."* Cycle 2 extracted
  `resolve_project_scope` out of `index_catalog` specifically to create the seam, and tested
  through it.

See also [prove-test-fails-without-fix.md](prove-test-fails-without-fix.md) — mutating the
guard is how you find out which layer your test is actually holding — and
[run-all-targets-clippy-before-trusting-a-lint-fix.md](run-all-targets-clippy-before-trusting-a-lint-fix.md),
whose closing lesson ("if a test has to route around the function it claims to cover, it is
testing something else") is the same instinct applied to a lint fix rather than to an
acceptance criterion.
