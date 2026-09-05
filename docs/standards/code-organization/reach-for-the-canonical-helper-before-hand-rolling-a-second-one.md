# Search for the canonical helper before writing the second implementation

A gate, a count, a status message: you need one at the site you are editing, the logic is four
lines, and you write it. The crate already had a named, documented helper for exactly that —
you did not find it because you never looked, and you never looked because the thing you were
about to write was too small to feel like it needed a search.

The cost is not the duplication. It is that a hand-rolled second implementation is almost
always *narrower* than the helper it shadows. The helper accumulated its extra behaviour one
incident at a time — a `tracing::warn!` on the failure arm added after a silent outage, an
`is_archived` predicate added after a count disagreed with the UI, a warnings field wired into
a poller the frontend already reads. Your four lines reproduce the happy path faithfully and
silently drop all of it, because none of it was visible from the call site you copied the
shape from. The result reads as correct at review, passes its own tests, and quietly re-opens
whichever bug the helper's extra clause was written to close.

The search that finds it is not a grep for the expression — you cannot grep for an expression
you have not written yet. Grep for the *concept* and for the type it operates on: the
permission enum, the table the count reads, the status writer the frontend polls. One
already-correct call site is enough to find the helper, and this codebase almost always has
one.

**Rule:** Before hand-writing a predicate, gate, count query, or status/warning emission,
grep for an existing helper by concept and by the type it consumes, and read one existing call
site. If a canonical helper exists, call it — even when your own version would be shorter.
If you decide to write a second implementation anyway, diff it against the helper clause by
clause and state in the PR body which of the helper's behaviours you are deliberately not
reproducing. Tests written against your copy do not close this gap: they pin the copy, and
make it look better covered than the surface it gates.

The WRONG block below is a reconstruction of the shape KYO-231 shipped, not a quote — the
function was deleted in that ticket's cycle 2 and has no committed form to quote:

```rust
// WRONG — a second implementation of the crate's permission lookup, written at the
// one gate that needed it. Fails closed like the helper does, but silently: the
// helper's warn! on a failed UserContext fetch has no counterpart here, so "every
// permission-gated surface vanished" loses its diagnostic trail at this gate only.
fn can_transfer_ownership_from(
    user_ctx: Option<Result<UserContext, ServerFnError>>,
) -> bool {
    match user_ctx {
        Some(Ok(ctx)) => ctx.permissions.contains(&Permission::TransferOwnership),
        Some(Err(_)) => false,
        None => false,
    }
}
```

```rust
// RIGHT — the canonical helper, which already fails closed, already distinguishes
// "loading" from "fetch failed", and already logs the second one.
let perms = use_permissions();
let can_transfer_ownership = Memo::new(move |_| perms.can(Permission::TransferOwnership));
```

Real precedent — three tickets in four days, each one a second implementation that was
narrower than the helper it shadowed:

- **KYO-231's `permission gates (billing.rs, settings_shell.rs, team.rs)` review** — 🟡
  Copy-Paste, refused signing. `can_transfer_ownership_from` in `team.rs` reimplemented
  `crate::utils::permissions`' `permissions_from` + `Permissions::can`, described by the
  reviewer as "the crate's own documented 'single lookup helper every UI gate should use'",
  with near-identical prior art already at `sql_editor/sidebar.rs` and two sites in
  `settings/datasources.rs`. The finding names the narrowing precisely: "the duplicate also
  silently drops the `tracing::warn!` on fetch failure that `permissions_from` has — a real
  (if minor) observability regression for this specific gate." The `re-review, cycle 2` entry
  records the resolution: the function, its doc comment **and its four dedicated tests** were
  deleted and the gate became a two-line `use_permissions()` call, with the reviewer
  confirming the deleted tests "were redundant with this, not a net coverage loss" — the
  coverage had been pinning the copy, not the behaviour.
- **KYO-615's `canonical table-count accessor (list_datasources vs browse_catalog)` review**
  shows both directions in one diff. Routing `get_public_table_count` and
  `get_sample_table_count` through the canonical `datasource_service::count_tables_for_workspace`
  made them "pick up the `is_archived` filter they were missing too", and replaced bare
  `.unwrap_or(0)` degradation with a `tracing::warn!` first — behaviour the hand-rolled
  counters never had. The same review's Notes flag the copy that remains: a hand-rolled
  `COUNT(*) … WHERE datasource_config_id = $1 AND is_archived = …` in
  `kyomi-ui/src/server_fns/datasources.rs`, correct today but "a second hand-written copy of
  the same predicate the ticket set out to eliminate."
- **KYO-658's `column-embedding FK-violation race` review** — 🟡, the same failure applied to
  a reporting channel rather than a predicate. The diff folded a column-embedding shortfall
  into `result.errors`, which the reviewer traced end to end and found "write-only": none of
  the four real callers read it. The project already had "a purpose-built, already-wired,
  already-polled channel for exactly this 'completed but with issues' case" —
  `catalog::helpers::update_datasource_status`'s `warnings` parameter, which the Settings
  page's poller already reads — so a user watching a refresh still saw a clean "completed",
  which is the exact failure the ticket existed to fix.
- **KYO-616's `Instrument catalog enumeration and archive decisions` review** is the same
  shape caught early, at 🟢: a `capture_tracing_for_test()` wrapper duplicated verbatim
  across two test modules, where "the fix belongs conceptually inside
  `kyomi_test_tracing::capture_tracing()` itself so every current/future caller gets it for
  free."

Distinct from
[propagate-predicate-changes-to-every-copy.md](propagate-predicate-changes-to-every-copy.md):
that rule triggers when you *edit* a predicate that already exists inline at several sites,
and its remedy is to extract and route every site through the extraction. This one triggers
before any of that — a canonical helper already exists, and the defect is writing copy number
two at all. Distinct from
[close-the-class-by-making-the-wrong-call-uncallable.md](close-the-class-by-making-the-wrong-call-uncallable.md),
which is about a *wrong* API left legal beside a new right one: here nothing is wrong or
deprecated, the right helper is the only one there is, and the author simply did not find it.
Distinct from
[third-copy-of-test-helper-is-extraction-trigger.md](third-copy-of-test-helper-is-extraction-trigger.md),
which sets the threshold at which duplication is worth extracting: this rule is about not
creating the duplication when the extraction already exists.
