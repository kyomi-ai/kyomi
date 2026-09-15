# An expected value read off the current behaviour guards the defect, not the requirement

A test reads a real computed value and asserts it equals a specific literal. The assertion is
not tautological — a regression genuinely would change that value and turn the line red. The
fixtures vary along the axis under test. The test name names the guarantee. It is still
worthless, and worse than worthless, because the literal on the right-hand side was obtained
by **running the code and writing down what came out**, rather than derived from what the code
is supposed to produce. When the observed behaviour is the defect, the assertion pins the
defect in place: fixing the bug makes the test fail, so the test now argues against its own
fix.

This is the expectation-side member of the vacuous-test family, and it is the one that looks
most like real coverage. Every other check in this section passes:

- it is not `an-assertion-a-constant-already-satisfies-cannot-fail` — this assertion *can*
  fail, and would, the moment the value changes;
- it is not `choose-inputs-the-broken-code-would-answer-differently` — the input is right, and
  it is exactly the input the bug corrupts;
- it is not `a-mutation-that-did-not-run-is-not-evidence` — mutate the production line and the
  test goes red on cue, which reads as proof that the assertion is load-bearing.

What the mutation proves is that the test is pinned to *something*. It cannot tell you whether
that something is the requirement or the bug. Only re-deriving the expected value from the
spec can.

The tell is a comment explaining *why* the observed value is what it is. A test written from
the requirement states the requirement. A test written from the output explains the output —
and the explanation is usually correct, which is what makes it persuasive. The KYO-702
instance below narrates the mechanism accurately and completely, and every sentence of it is
true; it simply describes a credential being destroyed.

**Rule:** Derive every expected value from the behaviour the code is required to have, not
from the behaviour it currently has. Before writing `assert_eq!(actual, <literal>)`, state in
one sentence why that literal is *correct* — not why the code produces it. If the only answer
available is "that is what it returns today", the test is a characterization test: say so in
its name, and file the question of whether the behaviour is right. Be most suspicious when the
literal is a sentinel — a mask, a placeholder, an empty string, a default — since those are
precisely the values a broken restore path leaves behind.

## WRONG

```rust
// oauth_client_secret is NOT a COMMON_SENSITIVE field, so
// finalize_connection_config_secrets never touches it — it must
// still hold the client's submitted value (the masked placeholder,
// in this test) because strip_inactive_auth_mode_fields correctly
// left the active mode's own field alone.
assert_eq!(incoming["oauth_client_secret"], MASKED_VALUE);
```

Every clause is factually accurate about the code as written. The assertion nonetheless
requires that the literal `"********"` be what gets persisted over a working credential.

## RIGHT

```rust
// The form round-tripped the mask, so the *real* stored secret must
// survive the save untouched. Anything else — including the mask
// itself — is the credential being destroyed.
assert_eq!(incoming["oauth_client_secret"], existing["oauth_client_secret"]);
```

## Real precedent

`strip_leaves_active_modes_own_fields_untouched_regardless_of_value`
(`crates/kyomi-auth/src/credential_service.rs`, added by **KYO-702**) asserted
`incoming["oauth_client_secret"] == MASKED_VALUE` after a save that round-trips the masked
placeholder. The review of that PR (`docs/review-logs/2026-09-15.md`, the KYO-702 entry headed
"BigQuery/Snowflake auth-mode-switch secret leak (strip_inactive_auth_mode_fields)", finding
#1 🔴, unsigned) found that the assertion documents a real data-loss bug as expected
behaviour: "the diff's own new AC3-labeled test directly exercises this exact scenario and
reports success while persisting a corrupted value — the test suite gives false confidence
about the very guarantee the review brief asked to confirm."

The same entry's finding #3 (🟢) recorded the smaller version of the same problem in the test's
name and comment, and the follow-up ticket **KYO-780** carries rewording it as an explicit
acceptance criterion alongside the fix. The test was written last, from the output of a
function the author had just read carefully — which is exactly the position from which the
observed value is most convincing and least questioned.

## Distinct from

- [`an-assertion-a-constant-already-satisfies-cannot-fail.md`](an-assertion-a-constant-already-satisfies-cannot-fail.md)
  — there the assertion cannot fail at all; here it can, and pins the wrong value.
- [`choose-inputs-the-broken-code-would-answer-differently.md`](choose-inputs-the-broken-code-would-answer-differently.md)
  — that rule is about the fixture agreeing by accident; this one is about the expectation
  being copied from the defect.
- [`prove-test-fails-without-fix.md`](prove-test-fails-without-fix.md) — a test can fail
  without the fix and still be asserting the wrong thing, when the "fix" it is pinned to is
  the bug.
