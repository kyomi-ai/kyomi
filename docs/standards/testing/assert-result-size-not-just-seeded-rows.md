# A test that reads back only its own seeded rows cannot catch a widened filter

A test that seeds two sessions, calls `fetch_session_counts(&[a, b])`, and asserts `counts[a] == 3` proves the rows it seeded are found. It says nothing about what else came back. Widen the query's `WHERE session_id = ANY($1)` to `... OR pinned = true` and every assertion still passes — the seeded ids already matched the id-list clause — while the caller now receives every other tenant's rows. That is exactly the shape of a scoping bug, and it is the one shape this kind of test structurally cannot see.

**Rule:** Any test over a query whose correctness depends on a filter must assert the *size* of the result, not merely the presence of the rows it seeded, with a message naming the leak the assertion exists to catch. Mutate the filter to the *widened* form (`AND` → `OR`) rather than to a broken one — a mutation that empties the result is killed by any assertion at all and proves nothing about scoping.

```rust
// WRONG — survives WHERE session_id = ANY($1) OR pinned = true
assert_eq!(counts.get("session-a"), Some(&3));

// RIGHT — the widened filter now fails on the count
assert_eq!(counts.len(), 2, "query must return only the requested sessions; an AND→OR widening would leak other workspaces' rows");
assert_eq!(counts.get("session-a"), Some(&3));
```

Flagged as 🟡 in KYO-292 (2026-08-09): `postgres_fetch_session_counts_counts_messages_and_pinned` (`crates/kyomi-auth/src/chat_service.rs`) and `postgres_fetch_member_counts_counts_only_active_members` (`crates/kyomi-auth/src/workspace_service.rs`) both looked up only their own seeded ids and both survived the `AND`→`OR` mutation traced above, while the two sibling tests in the same diff that did assert `.len()` did not. This is the test-side counterpart of *`workspace_id` is not an authorization boundary* above — the query returns more than the requester may see, and every assertion still passes.
