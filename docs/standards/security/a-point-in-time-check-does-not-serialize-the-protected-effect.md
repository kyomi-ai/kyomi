# A point-in-time check does not serialize the protected effect

An authorization check can return the correct answer and still fail a concurrency
guarantee. Another task can change the authorization state between the check and the
protected write or send. Putting a second check immediately before an awaited effect
narrows the window; it does not remove it.

**Rule:** When the contract requires an authorization change to serialize with a
mutation, or requires completed revocation to prevent subsequent old-generation work,
identify the ordering boundary shared by the protected operation and the state change.
Hold the relevant database row lock through authorization, write, and commit, or use
an explicit transport/lifecycle protocol that prevents revocation from reporting
completion while the prohibited send can still finish. A transaction alone does not
establish that ordering if the authorization read does not take the required lock.
Another pre-send check alone does not establish it either.

Test the overlap deterministically with a barrier or held operation. Arrange for the
authorization change to win the ordering, then release the blocked operation and
assert the actual mutation or transport outcome. Cover the admitted control as well.
For a transport, account for cancellation and buffered writes when defining completion;
dropping an awaited send is not automatically proof that no bytes can later flush.

WRONG — illustrative ordering, not a source quotation:

```text
operation: read authorization = allowed
operation: pause before protected effect
revoker:   change authorization and report complete
operation: resume and perform protected effect using earlier authorization
```

RIGHT — illustrative database ordering, not a source quotation:

```text
revoker:   lock the authorization row, change it, commit
operation: acquire the same row lock in its transaction
operation: read the current authorization under that lock
operation: deny without mutating when authorization was revoked
operation: otherwise retain the lock through the write and commit
```

Two initial security findings in `docs/review-logs/2026-09-27.md` instantiate this
pattern, and both fixes have landed:

- **KYO-847**, `private collection mutation authorization, initial review`: a
  junction-row INSERT/DELETE checked collection visibility through a separate table
  read. A concurrent public-to-private update could commit while the mutation continued
  under its earlier snapshot. The re-review records `lock_collection_for_mutation_pg`
  retaining the collection row lock through the write and commit, with PostgreSQL
  concurrency regressions for add and remove. Landed as `a8597c0b` (PR #573).
- **KYO-841**, `Connect session revocation`, initial review: revocation could finish
  between the final current-session check and WebSocket send, while the send branch
  could not observe revocation until that send completed. Subsequent re-reviews record
  the check/send race and canceled-send flush finding as resolved. Landed as `825adf63`
  (PR #577).

This rule applies to the stated serialization/completed-revocation contract. A path
whose documented contract is point-in-time authorization should state that boundary
accurately; this rule does not imply already transmitted bytes can be retracted.

Distinct from
[preserve-side-effect-and-error-ordering.md](../code-organization/preserve-side-effect-and-error-ordering.md):
that rule preserves the sequential order during a refactor; this rule requires an
ordering boundary between concurrent authorization changes and protected effects.
Distinct from
[unused-security-helper-worse-than-none.md](unused-security-helper-worse-than-none.md):
here the production check runs and answers correctly, but its answer can become stale
before the effect. The lifecycle rule
`data-state-management/replacement-does-not-complete-the-replaced-resources-teardown.md`
addresses a different KYO-841 finding: retaining and enumerating superseded resources
until cleanup completes, rather than the initial check/send race described here.
