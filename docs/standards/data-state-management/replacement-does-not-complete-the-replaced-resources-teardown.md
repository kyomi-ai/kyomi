# Replacement does not complete the replaced resource's teardown

A routing registry answers which connection should receive the next command. It does
not enumerate every connection that still owns a transport, pending work, or remote
registration. Replacing its entry can leave the outgoing handler alive. A stale-owner
guard must protect the replacement without discarding the outgoing handler's cleanup.

**Rule:** Track teardown identity for each resource independently of the current routing
entry. Keep that identity until the resource's transport and pending work are torn down
and its remote cleanup succeeds. When an outgoing resource unregisters, conditionally
remove shared routing/presence only if it still owns them, but always clean up its own
registration. A wait for teardown must enumerate all matching lifecycle records,
including superseded resources, rather than only the current route. Retain cleanup
identity across retryable cleanup failures.

WRONG — illustrative algorithm, not a source quotation:

```text
unregister(outgoing):
    if current_route != outgoing:
        return
    remove current_route
    remove outgoing's remote membership

wait_for_teardown(generation):
    wait until current_route no longer names generation
```

RIGHT — illustrative algorithm, not a source quotation:

```text
unregister(outgoing):
    look up outgoing's own lifecycle record
    remove current_route only if outgoing still owns it
    remove outgoing's remote membership
    compare-and-delete shared presence using outgoing's owner identity
    retain the lifecycle record if remote cleanup needs retry
    otherwise remove the lifecycle record

wait_for_teardown(generation):
    wait until all matching local and remote lifecycle records are gone
```

Test the replacement ordering explicitly: register A, replace it with B, keep A's
transport teardown blocked, and confirm teardown completion remains pending. Then
release A and verify its cleanup leaves B's route and presence intact. Test cleanup
failure separately so a failed remote operation cannot discard the identity needed
for retry.

Two distinct findings in **KYO-841's `Connect session revocation` re-reviews**
(`docs/review-logs/2026-09-27.md`) motivated this rule:

- Cycle 2, State Management: stale unregister returned before removing a superseded
  connection's remote active-set membership. The replacement was protected, but the
  outgoing resource's cleanup was skipped.
- Cycle 3: the local-only teardown wait inspected only the current routing entry and
  omitted a superseded handler that still had pending or in-flight work.

Both fixes landed in commit `825adf63` (PR #577). The concrete implementation is
`ConnectRegistry::unregister` and the per-connection `lifecycle` map in
`crates/kyomi-datasource/src/connect/registry.rs`; regression coverage includes
`local_revocation_waits_for_superseded_old_jti_handler` and
`superseded_same_and_new_jti_sessions_clear_only_their_own_membership`.

Distinct from [split-a-value-that-answers-two-questions.md](split-a-value-that-answers-two-questions.md),
which diagnoses overloaded state generally: this rule specifies the cleanup ordering
and completion evidence required after a resource loses routing ownership. Distinct from
[teardown-clears-the-whole-derived-state-group.md](teardown-clears-the-whole-derived-state-group.md),
which enumerates derived signals to reset; here the outgoing resource must retain its
own lifecycle record even after a replacement owns the shared key.
