# A teardown site clears the whole derived-state group, not the subset it happens to remember

When one input produces several pieces of derived state — a validation verdict, a status
enum, a discovered-options list — every site that invalidates that input has to clear *all*
of them. Clearing a subset is not a partial fix: the surviving signals are now stale
state attributed to credentials that no longer exist, and the UI keeps rendering them as
current.

This fails silently because each teardown site is written in isolation, months apart, by
whoever was fixing the bug in front of them. The signals that get cleared are the ones that
bug touched. Nobody re-reads the sibling teardown sites to check the sets match, so the
group is only ever cleared *completely* by accident.

**Rule:** When you add or edit a teardown/reset site, enumerate every signal derived from the
input being torn down and confirm each one is cleared — then grep for the sibling teardown
sites of the same input and confirm they clear the identical set. If the sets differ, one of
them is wrong; the divergence is the finding. Deriving the group from a single source (one
struct, one reset helper) is better than N hand-maintained reset blocks, and is the right fix
once a third site appears (see
[third-copy-of-test-helper-is-extraction-trigger](../code-organization/third-copy-of-test-helper-is-extraction-trigger.md)
for the same threshold applied to helpers).

A mode/selector change counts as a teardown. Switching BigQuery's Authentication Mode
invalidates the previous mode's validation just as surely as deleting the credentials does,
but an `on_change` that only calls `set_mode.set(val)` does not read as a teardown site to
whoever writes it.

WRONG — the Remove chip clears two of the three signals its credentials produced:

```rust
// service-account "Remove" chip
on:click=move |_| {
    set_service_account_email.set(String::new());
    set_cfg_service_account_json.set(String::new());
    set_test_result.try_set(None);
    set_discovery_status.try_set("idle".to_string());
    // bq_projects still holds the GCP projects those credentials discovered,
    // and BqProjectField keeps rendering them as selectable options.
}
```

RIGHT — clears the same set its sibling teardown sites clear:

```rust
on:click=move |_| {
    set_service_account_email.set(String::new());
    set_cfg_service_account_json.set(String::new());
    set_test_result.try_set(None);
    set_discovery_status.try_set("idle".to_string());
    set_bq_projects.try_set(vec![]);
}
```

Flagged in KYO-413 (2026-08-21), which produced two findings of this exact shape in a single
review — a PR whose whole purpose was fixing one instance of it:

- The service-account "Remove" chip (`datasources.rs:5059-5074`) cleared `test_result` and
  `discovery_status` but not `bq_projects`, while its two sibling teardown sites
  (`google_disconnect_action` and `datasource_disconnect_action`, `:2423`/`:2459`) both
  explicitly cleared all three. Result: a stale list of previously-discovered GCP projects
  stayed rendered as `<Select>` options after the credentials that produced them were removed.
- The Authentication Mode selector's `on_change` (`datasources.rs:4809`) cleared nothing, so
  validating under `kyomi_oauth` and then switching to `service_account` left
  `test_result.success == true` and held the Next/Catalog gate open for a mode that
  had never been validated.

The same group discipline appears from the other direction in KYO-469 (2026-08-23): removing
the `discovery_error` signal required finding and deleting all of its setters, its resets, and
its reader — five sites for one signal. If deleting a member of the group takes an exhaustive
sweep, so does clearing one.
