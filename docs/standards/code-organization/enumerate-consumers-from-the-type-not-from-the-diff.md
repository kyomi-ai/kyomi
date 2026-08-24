# Enumerate a change's consumers from the type, not from the diff

When a *producer* changes — a struct field is renamed, a response type gains a new
channel, a stored value gains a new leaf — the sweep of affected sites must be derived
by grepping the **producer's name across the whole workspace and the sibling repos**.
Deriving it from the files the ticket named, or from the set already open in the diff,
reliably produces an N that is one short.

The characteristic tell is a PR that states a count: *"all six boolean leaves"*, *"the
six-file rename sweep"*, *"both consumers"*. Every one of those sentences is an
enumeration made from what the author was looking at, and the review finding is always
the same shape — a seventh site, in a file the ticket never mentioned.

Three things make the miss invisible at author time:

- **The compiler is not a backstop.** `enterprise/kyomi-slack` is an unconditional
  workspace member but an *optional* dependency of `apps/server` behind the `slack`
  feature, so a narrowed `cargo check -p <crate>` — the scoping this repo's build-cost
  guidance actively encourages — never touches it.
- **The other consumer contains none of the new text.** You cannot grep for the thing
  you added: the blind sibling reads the *old* field and looks completely healthy. Only
  a grep for the shared producer finds it.
- **A mechanical fix can compile and still be wrong.** Renaming a field at the missed
  call site clears the build error while silently changing that path's behaviour, if the
  rename also changed what the field *means*.

**Rule:** Before claiming a sweep is complete, grep for the producer's identifier — the
struct, the field, the response type, the config key — across `apps/`, `crates/`,
`enterprise/`, **and** `~/repos/kyomi-connect` (see *Where things live* in `CLAUDE.md`;
a workspace-only grep silently misses the drivers). Then, for each site found, ask
whether it wanted the old *behaviour* or merely the old *name* — a rename that also
changes meaning must not be applied mechanically. State the enumeration command in the
PR, not the resulting number: a reviewer can re-run a grep, and cannot re-derive a count.

```rust
// WRONG — the sweep is the set of files already open.
// "Renamed AgentExecutionConfig::user_message_id across all 6 call sites."
// `cargo check -p kyomi-agent` passes. `--features slack` does not.

// RIGHT — the sweep is whatever the producer's name returns.
//   grep -rn "user_message_id" --include='*.rs' apps/ crates/ enterprise/ ~/repos/kyomi-connect/
// ...finds a 7th site in enterprise/kyomi-slack/src/routes.rs, and asking what that
// site *wanted* shows a mechanical rename would have broken it — so the signal was
// split into a distinct type instead of overloading one field:
pub enum UserMessagePersistence { /* crates/kyomi-agent/src/adapter.rs */ }
```

Three findings on 2026-08-23, three different tickets, one shape:

- **KYO-492** (🔴, `00:58`) — `AgentExecutionConfig::user_message_id`
  (`crates/kyomi-agent/src/execution.rs`) was renamed across a declared six-file sweep;
  a seventh construction site at `enterprise/kyomi-slack/src/routes.rs` was missed and
  the diff did not compile under `--features slack`. The review's second 🔴 is the more
  instructive one: the field had quietly taken on a second meaning ("already persisted,
  skip re-persisting"), so a find-and-replace fix would have compiled *and* silently
  stopped persisting every Slack user message. Shipped resolution split the two signals
  into `UserMessagePersistence` (`crates/kyomi-agent/src/adapter.rs`) rather than
  renaming in place.
- **KYO-466** (🟡, `17:22`) — `DiscoverResourcesResult`
  (`crates/kyomi-ui/src/server_fns/datasources.rs`) gained a `resource_errors` map.
  `EditModeCatalogTab` consumed it; `test_action`'s Effect — the sibling consumer of the
  same struct, in the same file — kept reading only `resources` and still rendered an
  unexplained empty dropdown, the exact failure the ticket existed to fix. Resolved by
  folding the second consumer into the same PR.
- **KYO-460** (🟡, `14:00`) — a migration retyped the corrupted `connection_config`
  boolean leaves; the PR's *"six is the complete set"* was empirically false.
  `shared_credentials`, written by the same pre-KYO-428 code path and read by
  `resolve_shared_credentials` in `~/repos/kyomi-connect`, was absent from both the
  Postgres and SQLite migrations. Only a grep that left this repo would have found it;
  both shipped migrations now cover it.

Sibling of [propagate-predicate-changes-to-every-copy.md](propagate-predicate-changes-to-every-copy.md):
that rule is about one expression duplicated verbatim, so grepping the *expression*
finds every copy. This one is the case where that technique cannot work — the sites you
are missing do not contain the new text, so the producer's name is the only handle.
See also [audit-write-sites-when-tightening-constraint.md](../data-state-management/audit-write-sites-when-tightening-constraint.md)
for the same discipline applied to a table's write sites.
