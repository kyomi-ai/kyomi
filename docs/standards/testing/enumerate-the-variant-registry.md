# A per-variant behaviour needs a test that enumerates the variant registry — not a test of the variant in front of you

When behaviour is implemented once per provider/mode/variant, and each variant reaches that behaviour by its own route, testing the variant you happen to be working on proves nothing about the others. A hand-typed list of "the variants I tested" is a second source of truth for something that already has one — the registry — and it drifts the moment someone adds a variant without also remembering to extend the list. The failure is silent: the new variant's tests are simply absent, the suite stays green, and nothing distinguishes "not yet needed" from "forgotten."

**Rule:** When a behaviour is per-variant (per datasource provider, per auth mode, per any other registry-backed enum), write the test's pair set as a derivation from the registry itself — `all_metadata()`, `EnumIter`, a `match` with no wildcard arm, whatever makes an unhandled variant a compile error or a runtime failure — never as a literal list typed into the test. A variant added to the registry with no corresponding entry in the test's own mapping must fail loudly, naming the variant, not silently pass by falling through a catch-all.

```rust
// WRONG — a hardcoded list that already existed before the new variant
// shipped; nothing forces it to grow when the registry does
#[test]
fn each_provider_reaches_next() {
    for (provider, mode) in [("snowflake", "password"), ("postgres", "password")] {
        assert!(has_route(provider, mode));
    }
}

// RIGHT — derived from the registry; a new (provider, mode) pair with no
// route fails here immediately instead of shipping silently
#[test]
fn every_registry_pair_has_a_route() {
    for (type_id, meta) in kyomi_core::datasource_registry::all_metadata() {
        for mode in meta.auth_modes {
            assert!(
                expected_route_for(type_id, &mode.mode_id).is_some(),
                "no route recorded for {type_id}/{} — a provider or auth mode was added \
                 with no wired path to enable Next",
                mode.mode_id
            );
        }
    }
}
```

Real precedent: KYO-404/405 — BigQuery was uncreatable through the UI in all three of its auth modes for months because every *other* provider's route to the create wizard's "Next" button happened to work, and nothing enumerated the (provider, mode) matrix to notice BigQuery's was missing; a paying customer reported it, not CI. Same shape as the SSH-tunnel regression (KYO-123/124/125): a behaviour silently not ported/wired for one variant, with nothing in CI positioned to notice because the test suite only ever exercised the variants someone remembered to list.
