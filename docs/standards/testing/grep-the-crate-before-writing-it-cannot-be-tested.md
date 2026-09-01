# "It can't be tested here" is a claim about the crate, and the crate can be grepped

Every other assertion in a PR body invites scrutiny. This one does not, because it is
phrased as an admission: *no seam to inject without mocking*, *there's no tracing-capture
dependency available*, *nothing in this environment can make that call fail*. It reads as
candour — the author volunteering a weakness — and candour is the thing reviewers are
trained to reward. So the sentence sails through as a disclosure when it is actually a
factual claim about a small, enumerable, three-grep-deep thing: what is in this crate's
`[dev-dependencies]`, what helpers already sit in its `mod tests`, and what constraints the
schema already enforces on the path under test.

The claim is usually made from memory, at the end of the work, about a crate the author has
only read the middle of. And it fails in a uniquely quiet way. A fabricated identifier fails
loudly the first time someone greps for it; a wrong "can't be tested" produces a green suite,
an honest-sounding paragraph, and a shipped behaviour change with no assertion behind it.
Nothing in CI disagrees. The cost lands entirely on the review cycle: a reviewer spends the
three greps the author didn't, and the diff comes back.

Note the shape of the two real refutations below. Neither reviewer had to design a test — the
harness was already sitting in the same crate, already used for the same class of assertion,
already constructed mock-free two files away. Neither refutation took longer than reading the
`Cargo.toml`.

**Rule:** Before you write that something cannot be tested, run the checks that would refute
you, and record what they returned:

1. **The dependency.** `sed -n '/\[dev-dependencies\]/,/^\[/p' crates/<crate>/Cargo.toml` — is
   the harness you say you lack already there? (`kyomi-test-tracing` is in more crates than
   you think.)
2. **The precedent.** Grep the crate for the assertion style you need — `capture_tracing`,
   `test_pool`, `seed_user`, a `::new(None, db.clone())` — and see whether a sibling module
   already does this without a mock.
3. **The real constraint.** Ask what the production path already enforces that you could
   violate with genuine data — a foreign key, a unique index, a `NOT NULL` — before reaching
   for an injection seam at all. A real FK violation is a real error, not a mock.

If any check refutes the blocker, there is no blocker. If they all come back empty, say
*which ones you ran* — an unchecked "can't" is a guess wearing a disclosure's clothes, and
per
[../version-control-working-tree/state-the-acceptance-criterion-you-did-not-meet.md](../version-control-working-tree/state-the-acceptance-criterion-you-did-not-meet.md)
the disclosure is what earns a genuine gap its pass. And when a reviewer refutes one such
claim, re-run all three before you write the next one: the refuted claim is evidence about
your inventory of the crate, not just about that one sentence.

```rust
// WRONG — the AC is "the failure now logs at error! with session_id and
// assistant_message_id". The test asserts the function didn't panic and the
// row is absent, and the PR body explains why the log itself is unreachable:
//
//   "No tracing-capture dev-dependency is available in this crate."
//
// Both halves are false. The test passes identically against the pre-fix
// `let _ = add_message(...).await;`, so it cannot fail without the fix.
#[tokio::test]
async fn save_agent_error_survives_a_genuine_fk_violation_on_the_fallback_insert() {
    save_agent_error(params_with_unseeded_session_id()).await;
    assert_eq!(message_count(&db).await, 0);
}

// RIGHT — one grep of crates/kyomi-auth/Cargo.toml finds
// `kyomi-test-tracing = { workspace = true }`; one grep of the crate finds
// `capture_tracing()` already doing this in mcp_session_manager.rs
// (`validate_session_absent_does_not_log_error`) and auth_service.rs.
// The FK violation stays — it is the real failure, mock-free — and the
// assertion moves onto the emission the criterion actually names.
#[tokio::test]
async fn save_agent_error_logs_the_fallback_insert_failure() {
    let logs = kyomi_test_tracing::capture_tracing();

    save_agent_error(params_with_unseeded_session_id()).await;

    let errors = logs.events_at(Level::ERROR);
    assert!(
        errors.iter().any(|(_, msg)| msg.contains(SESSION_ID) && msg.contains(ASSISTANT_MSG_ID)),
        "the fallback insert failure must log both ids; captured: {errors:?}"
    );
}
```

Both refutations come from one ticket, in consecutive cycles — the second written *after* a
reviewer had already disproved the first:

- **KYO-579 cycle 1** (review log `2026-09-01`, `20:35`, 🟡) — the stated reason for shipping
  `save_agent_error`'s new error branch untested was "no seam to inject without mocking". The
  reviewer: *"The implementer's stated reason ... is incorrect"* — `chat_messages.session_id`
  has a real FK to `chat_sessions`, enforced on both engines, and this file's own `test_pool`
  already runs `PRAGMA foreign_keys=ON`. Calling `save_agent_error` with a `session_id` never
  seeded into `chat_sessions` makes the insert fail for real. The finding also notes the
  scaffolding was already sitting in the same `mod tests` — `test_pool`, `seed_user`, and a
  mock-free `WebSocketManager::new(None, db.clone())` used that way in `collection_service.rs`
  and `watch_service.rs`.
- **KYO-579 cycle 2** (`2026-09-01`, `20:50`, 🟡) — the FK test landed, but the log assertion
  did not, because "no tracing-capture dev-dependency [is] available, told not to add one."
  The reviewer: *"factually incorrect: `kyomi-test-tracing` is already a `[dev-dependencies]`
  entry in `crates/kyomi-auth/Cargo.toml` and is already used twice in this exact crate for
  this exact class of assertion"* — `mcp_session_manager.rs`'s
  `validate_session_absent_does_not_log_error` and `auth_service.rs`. No new dependency was
  needed; cycle 3 (`21:00`) closed clean by wrapping the test already written in
  `capture_tracing()`.

The rule is not "never say it can't be tested" — it is "check first, then say what you
checked." The same week has the honest version, and it was explicitly not a finding:
**KYO-539** (`2026-08-30`, `09:15`) disclosed "no hook to force a real interleave through
`ModifyDashboardTool::execute`'s public surface", and the reviewer recorded that the "stated
gap ... is genuine and honestly documented — that gap is not itself a finding."

Distinct from
[cover-the-path-the-criterion-names-not-an-adjacent-one.md](cover-the-path-the-criterion-names-not-an-adjacent-one.md),
which covers the *layer* you tested — a pure helper or sibling handler standing in for the
path the criterion names — and whose remedy is to build a fixture on the real path or extract
a seam the real path goes through. That rule already treats environment excuses ("there is no
Postgres here") as checkable; this one narrows to the excuse's other family, where the missing
thing is claimed to be *test-support inventory* — a dev-dependency, a fixture, an existing
helper — rather than a reachable branch, and the check is a grep of `Cargo.toml` and
`mod tests` rather than a trace of the production path. Sibling of
[every-fix-ships-with-a-test.md](every-fix-ships-with-a-test.md): that rule says the test is
mandatory; this one covers the sentence written in its place. The failure family is
[../comments-documentation/a-resolving-identifier-is-not-a-verified-claim.md](../comments-documentation/a-resolving-identifier-is-not-a-verified-claim.md)
applied to a claim about tooling instead of a citation — plausible, unchecked, and trusted
precisely because it sounds like it was checked.
