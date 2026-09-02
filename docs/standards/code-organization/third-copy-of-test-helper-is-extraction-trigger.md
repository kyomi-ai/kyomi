# The third copy of a test helper is the extraction trigger — not the second

Two independent copies of a test helper can be justified ("different crate, not worth a shared dependency"). A third copy means the justification was wrong: the helper is general, and the copies will drift. By the third copy, each one has usually already been reviewed and approved individually, so nobody sees the aggregate.

**Rule:** Before writing a test helper, grep for its distinctive symbol names across `crates/` and `apps/`. If two copies already exist, extract all of them into a shared test-support crate (`kyomi-test-tracing`, `kyomi-test-harness`) in the same change rather than adding a third. If you decide against extracting, you must record the reasoning on the tracking ticket — a fresh "it's not worth it" comment in the new file, re-deriving a justification an earlier ticket already evaluated, is what makes the duplication invisible.

```rust
// WRONG — third inline copy, with a comment re-deriving the same justification
// as the two existing copies, and no reference to the ticket tracking them
struct CaptureLayer { /* ... */ }
struct EventLog { /* ... */ }

// RIGHT — one implementation, three consumers
use kyomi_test_tracing::capture_tracing;
let logs = capture_tracing();
assert!(logs.events_at(Level::ERROR).is_empty());
```

Flagged in KYO-240 cycle 1: the PR added a third `CaptureLayer`/`EventLog` copy after `kyomi-auth/src/mcp_session_manager.rs` and `apps/server/src/routes/auth_passkeys.rs`, which is the exact trigger condition KYO-244 had already written down. The extraction into `crates/kyomi-test-tracing` in cycle 2 was accepted as in-scope precisely because it was a direct response to a finding on that PR. The same class is still open elsewhere: duplicated SQLite fixture helpers in `kyomi-slack` and per-module seeding helpers in `kyomi-auth`. The `extract_between` instance noted here as "two settings test modules" was closed by KYO-272 — by then it had reached *ten* copies spread well beyond `settings/`, in two silently divergent variants, which is what an under-counted "only two copies" entry costs if it is left to age.
