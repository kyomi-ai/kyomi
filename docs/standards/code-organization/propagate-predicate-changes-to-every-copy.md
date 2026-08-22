# Changing a duplicated predicate means updating every copy — or extracting it

The same boolean predicate, reset set, or default is frequently spelled out inline at several sites: a footer's `can_next` and a tab pill's `class`/`disabled`/`on:click` guard, one reset block per credential-teardown handler, one absent-key default per consumer. When a ticket asks for a new exception or a changed default, a diff that applies it only at the site the ticket names *looks* complete and survives review-by-reading — the other copies are structurally identical and silently keep the old behaviour. The result is worse than not fixing it: half the feature works, so the user hits a control that is enabled on one path and permanently dead on another, with nothing in either site's source hinting that a sibling disagrees.

**Rule:** Before editing an inline predicate, reset set, or default, grep for the *whole expression* rather than trusting the ticket's line number. If it appears more than once, extract it in the same change — a `Signal<bool>`, a plain helper fn, a shared `*_default()` — and route every site through it, then add a guard test asserting the sibling sites contain no raw read of the underlying signal (or that the two implementations agree for the same inputs). Leaving two independent implementations "because one arm must not read `slug`" is not an exemption: if the duplication is genuinely required, the test must pin that the copies still decide identically.

```rust
// WRONG — the new exception reaches can_next only; three identical reads keep the old rule
let can_next = move || {
    !name.get().is_empty()
        && (test_result.get().map(|r| r.success).unwrap_or(false) || bq_enterprise_oauth_precreate())
};
// ...tab bar, 200 lines later:
class=move || if test_result.get().map(|r| r.success).unwrap_or(false) { TAB } else { TAB_DISABLED }
disabled=move || !test_result.get().map(|r| r.success).unwrap_or(false)
on:click=move |_| if test_result.get().map(|r| r.success).unwrap_or(false) { go_to_catalog() }

// RIGHT — one predicate, every site reads it
let connection_step_satisfied = Signal::derive(move || {
    test_result.get().map(|r| r.success).unwrap_or(false) || bq_enterprise_oauth_precreate()
});
let can_next = move || !name.get().is_empty() && connection_step_satisfied.get();
```

Flagged as 🟡 in KYO-404 (2026-08-21 13:20): the `bq_enterprise_oauth_precreate` exception was added to `can_next` (`datasources.rs:2812`) but not to the three identical `test_result.get().map(|r| r.success)` reads in the create-mode tab bar (`:3029,:3034,:3036`), so the footer's Next button advanced while the Catalog pill rendered permanently `TAB_DISABLED` and its `on:click` silently no-oped. Cycle 2 resolved it exactly as above, with a `catalog_tab_pill_shares_connection_step_satisfied_with_can_next` test pinning that the tab bar holds no raw `test_result.get()`. The same shape recurred twice more in the following day: KYO-413 (2026-08-21 21:32, two 🟡) — the auth-mode `Select`'s `on_change` (`:4809`) reset nothing and the service-account "Remove" chip (`:5059-5074`) reset `test_result`/`discovery_status` but not `bq_projects`, unlike its two sibling teardown Effects at `:2423`/`:2459`; and KYO-443 + KYO-426 (2026-08-22 08:40, 🟡) — the create-mode/empty-slug guard left as two independent implementations (`:4581-4591` and an inlined copy in the Memo at `:4677-4703`) whose replacement tests pinned each one's source text but never asserted the two decide alike. KYO-446 (2026-08-22 10:15) is the counter-example done right: the `include_public_datasets` absent-key default was flipped at all six consumer sites in one change, every one routed through a single helper.
