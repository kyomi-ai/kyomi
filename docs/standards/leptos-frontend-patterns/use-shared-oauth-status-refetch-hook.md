# Use the shared OAuth-status re-fetch hook when auth mode toggles

When a datasource modal supports multiple auth modes (e.g. `service_account` / `kyomi_oauth` / `enterprise_oauth`), the OAuth status panel must re-fetch status when the user switches modes. Without this, the panel shows stale data from the previously-fetched mode.

**Rule:** Any new provider's `*AuthModeSection` component must call the shared `use_oauth_status_refetch` hook, passing a mapping fn (`fn(&str) -> Option<OAuthStatusSource>`) that resolves the current auth mode to its OAuth status source; modes that don't use OAuth (e.g. `service_account`) map to `None`. Do not hand-roll another `Effect::new` for this — the `auth_mode_sections_do_not_hand_roll_oauth_status_effects` guard test fails the build if one is added.

```rust
// WRONG — hand-rolled Effect, now a guard-test failure
Effect::new(move |_| {
    let mode = bq_auth_mode.get();
    let slug_val = slug.get();
    if mode == "service_account" || slug_val.is_empty() { return; }
    set_oauth_connected.set(false);
    set_oauth_email.set(None);
    set_oauth_expired.set(false);
    spawn_local(async move {
        // fetch status for the correct mode...
    });
});

// RIGHT — shared hook + a per-provider mapping fn
fn bigquery_oauth_source(mode: &str) -> Option<OAuthStatusSource> {
    match mode {
        "kyomi_oauth" => Some(OAuthStatusSource::GoogleAccount),
        "enterprise_oauth" => Some(OAuthStatusSource::Datasource("bigquery-enterprise")),
        _ => None,
    }
}

use_oauth_status_refetch(
    bq_auth_mode,
    slug,
    OAuthStatusSetters {
        connected: set_oauth_connected,
        email: set_oauth_email,
        expired: set_oauth_expired,
    },
    bigquery_oauth_source,
);
```

This pattern was independently flagged in KYO-13 (BigQuery) and KYO-17 (Databricks) reviews, and recurred a third time in Synapse (KYO-197) because each fix copy-pasted the Effect instead of sharing it. That third recurrence is what motivated extracting `use_oauth_status_refetch`.
