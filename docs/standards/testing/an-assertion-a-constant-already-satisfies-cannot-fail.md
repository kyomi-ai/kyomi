# An assertion a constant in the code under test already satisfies cannot fail

A test reads back a real value produced by a real run — an HTTP body's `status` field, a
rendered log line — and asserts a property of it. The assertion looks like coverage: it names
the field the ticket is about, it carries a message stating the invariant, and it sits in a
test whose name is the acceptance criterion. It is still dead, because the property it checks
is supplied by a *literal* in the production code rather than by anything the run decided.

Two spellings produce it:

- **The predicate enumerates the closed set the code can emit.** `assert!(status ==
  "healthy" || status == "degraded")` against a field constructed as `if all_healthy {
  "healthy" } else { "degraded" }`. Every branch satisfies the disjunction, so no input,
  no fixture and no regression can make the line red.
- **The substring is already in the static half of the string.** `assert!(rendered.contains(
  "verification"))` against a log whose message is the constant `"Failed to send
  verification email"`. The variable half — the field that says *which* email — is never
  looked at, and the assertion passes for every value it could hold.

What makes this worth its own rule is that it survives the checks that catch the rest of the
vacuous-test family. The entry point is right, the assertion reads the computed value rather
than the wiring, the fixtures vary along the axis under test, and the mutated line really did
compile and execute. The value the assertion receives is genuinely the one the defect
corrupts — it is the *predicate* that was decided at authoring time. The only thing that
exposes it is running the mutation and noticing the run stayed green, which is exactly the
step the assertion's confident message discourages.

**Rule:** Assert on the part of the value the code under test can get wrong. Before writing
`assert!(x == A || x == B)` or `assert!(s.contains(LIT))`, go and read how the production
code builds `x` or `s`: if `A`/`B` are its only two arms, or if `LIT` lives in a format
string's literal half, the line is dead — pin the single value this path must produce
(`assert_eq!(status, "healthy", ...)`) or the interpolated field that carries the variation
(`rendered.contains(r#"email_kind="verification""#)`). A disjunction over a closed set and a
substring of a constant message are both authoring-time facts, not run-time observations.
Then mutate, per [prove-test-fails-without-fix.md](prove-test-fails-without-fix.md) — a
tautological assertion is one of the few defects a green mutation names immediately.

```rust
// WRONG — a reconstruction, not a quote: KYO-716 squash-merged as PR #518, so no
// `<sha>^` carries the pre-fix line; this is the form the 2026-09-12 review log
// records verbatim. `HealthResponse.status` is built by
// `apps/server/src/health.rs:88` as `if all_healthy { "healthy" } else { "degraded" }`,
// so both arms satisfy the disjunction and a drift-triggered flip to "degraded" —
// the regression the ticket exists to forbid, and the one that would have the
// readiness probe restart the process — leaves this green.
let status = body["status"].as_str().unwrap();
assert!(status == "healthy" || status == "degraded");

// RIGHT — `apps/server/tests/contract_health.rs:241-242` on `main`, verbatim.
// `build_state()` `.expect(...)`s on db and kv, so "healthy" is the one value this
// harness can legitimately produce, and the equality goes red on the mutation the
// disjunction slept through.
let status = body["status"].as_str().unwrap();
assert_eq!(status, "healthy", "drift must not change status");
```

Real precedent — two tickets, two days, two crates; one of them blocked signing:

- **KYO-716, `2026-09-12` log, the `09:19` entry "migration schema drift detection"** — 🟡,
  *Test Coverage*, `apps/server/tests/contract_health.rs:242`. The reviewer did not reason
  about it, they ran it: *"Verified by mutation: forced `status` to flip to `"degraded"`
  whenever `missing_versions` is non-empty — the exact regression the ticket forbids, which
  would trip the readiness/liveness probes and cause the outage KYO-716 exists to prevent —
  and this test still passed."* The `09:44` cycle-2 entry records the close-out, also by
  mutation rather than by reading: with `assert_eq!(status, "healthy", ...)` in place, the
  same patch made `health_schema_key_names_drifted_versions_without_changing_status` fail
  with `left: "degraded", right: "healthy"`, and the reviewer's note is that *"the test is
  now load-bearing"*.
- **KYO-697, `2026-09-14` log, the `01:41` cycle-2 entry "typed `EmailSendError` for email
  send observability"** — 🟢, *Test Coverage*, `crates/kyomi-auth/src/auth_service.rs:5419`.
  *"`assert!(rendered.contains("verification"), "the log must say which email failed")` is a
  tautology — the static message `"Failed to send verification email"` already contains
  `verification`, so it passes for every `VerificationEmailKind` and never actually pins the
  `email_kind` field."* Note the assertion message: it states the invariant the author
  believed they were enforcing, and it is the reason nobody re-read the predicate. The
  `01:56` cycle-3 entry closed it *by mutation, not inspection* — flipping the kind to
  `VerificationEmailKind::PasskeyRecovery` turned the rewritten
  `rendered.contains(r#"email_kind="verification""#)` red, which also settled how `tracing`
  renders the field.

Distinct from
[contains-after-assert-eq-is-dead.md](contains-after-assert-eq-is-dead.md), which is the
nearest sibling and was the rule cited in KYO-697's own fix comment: there the substring
guard is dead because an `assert_eq!` on the *same value* already decided the outcome one
line above, and the remedy is to drop it or re-point it at a different value. Here there is
no second assertion — the line is dead on its own, against a value nothing else asserts on,
and the remedy is to narrow the predicate until only the correct answer satisfies it. That
rule's closing move (pin a constant a future reword could break) is a legitimate *use* of a
compile-time property; this rule is about mistaking one for a run-time observation.

Distinct from
[assert-the-value-not-that-the-call-happened.md](assert-the-value-not-that-the-call-happened.md):
that rule is about an assertion aimed at the *wiring* — a source-text marker, an occurrence
count — which cannot see what the wiring is bound to. Both assertions here are aimed
squarely at the computed value; the defect is the shape of the question asked about it.

Distinct from
[choose-inputs-the-broken-code-would-answer-differently.md](choose-inputs-the-broken-code-would-answer-differently.md):
that one is input-side — the fixtures agree by accident, so a coarser rule sorts them
correctly and the axis under test is never reached. Change the fixtures and that test comes
alive. No fixture rescues a predicate that is true for every value the code can construct.

Distinct from
[a-mutation-only-counts-if-the-run-could-have-failed.md](a-mutation-only-counts-if-the-run-could-have-failed.md):
there the *run* was incapable of the outcome — the mutated body was never compiled, a stale
fingerprint was replayed, a different copy of the script executed. Both mutations here ran,
compiled the changed line, and reached the assertion. The run was capable; the assertion was
not.
