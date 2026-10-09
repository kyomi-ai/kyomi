# Every field you mask on read needs a restore path on write — from the same list

A datasource's `connection_config` is masked on the way out and replaced wholesale on the
way in. `mask_connection_config` swaps every sensitive field for `MASKED_VALUE`
(`"********"`), the settings form pre-fills from that response, and on save the client
sends the placeholder straight back for every secret the user did not retype. So the write
path has to know which incoming values are placeholders standing in for something already
stored — and that knowledge lives in a *list*.

The defect is that the mask side and the restore side read **different** lists. On read,
`mask_connection_config` consults the registry
(`DatasourceTypeMetadata::sensitive_connection_config_fields`) *and* the crate-local
`COMMON_SENSITIVE`. On write, `finalize_connection_config_secrets` consulted only
`COMMON_SENSITIVE`. Every type-specific secret — BigQuery's and Synapse's
`oauth_client_secret`, Snowflake's and Databricks' the same, BigQuery's
`service_account_json` — was masked on the way out and had no restore counterpart on the
way back, so the literal string `"********"` was written over the real credential. The save
returns 200, the settings page redraws with a masked field, and nothing is wrong anywhere a
reader can see until the datasource next tries to authenticate.

The same asymmetry has a quieter second face: a field the client never loaded back is simply
*absent* from the submitted map, and a wholesale replace with no restore path deletes it.
That is how BigQuery's `service_account_json` disappeared on any `service_account`-mode save
where the user did not re-upload the key.

And it runs in the other direction too. A field an auth mode declares it owns
(`AuthModeConfig::connection_config_fields`) but that its type's
`sensitive_connection_config_fields` omits is never masked at all: Snowflake's and
Databricks' `oauth_client_secret` was returned in cleartext by every settings read for years
because the two lists had simply never been kept in sync.

**Rule:** Drive masking and restoring from the same registry entry — the type's
`sensitive_connection_config_fields` plus the common list — never from a hand-written
constant beside whichever loop happened to need one. On write, treat *absent* and *equal to
`MASKED_VALUE`* as one case: restore from the stored config, and when there is nothing to
restore, drop the key rather than persisting the placeholder. When you add a sensitive field
to an auth mode's `connection_config_fields`, add it to that type's
`sensitive_connection_config_fields` in the same change. Before writing any new masking or
restoring loop, read the other side and compare the literal it iterates: if the two differ,
the difference is a live bug, not a scoping decision.

```rust
// WRONG — quoted from `crates/kyomi-auth/src/credential_service.rs` at `0f30cc5b^`.
// The read side masks registry fields *and* COMMON_SENSITIVE; the write side
// restores COMMON_SENSITIVE only, so a masked `oauth_client_secret` is persisted
// as the literal "********".
for &field in COMMON_SENSITIVE {
    let is_masked_or_absent = match incoming_obj.get(field) {
        Some(Value::Null) => {
            // Explicit clear — remove the field rather than restoring it.
            incoming_obj.remove(field);
            continue;
        }
        None => true,
        Some(Value::String(s)) => s == MASKED_VALUE,
        Some(_) => false,
    };
    // ... restore-from-existing / encrypt-fresh-plaintext ...
}

Ok(())

// RIGHT — the same file on `main` today, after KYO-780. The write side reads the
// same registry field the read side masks from, and restoration is additionally
// gated on the field being owned by the auth mode active in `incoming` so it
// cannot resurrect a secret KYO-702's strip step just removed on purpose.
if let Some(meta) = meta {
    let type_specific_fields = meta.sensitive_connection_config_fields;
    // ...
    let restorable: std::collections::HashSet<&str> = match active_auth_mode {
        Ok(Some(mode)) => { /* type_specific_fields minus this mode's inactive set */ }
        Ok(None) => type_specific_fields.iter().copied().collect(),
        Err(_) => std::collections::HashSet::new(), // retired/unresolvable: restore nothing
    };
    // ... same missing-or-masked → restore → else-encrypt rule as COMMON_SENSITIVE ...
}
```

Real precedent — one review, two 🔴, both pre-existing and both escalated rather than
deferred:

- **KYO-702**, review log `2026-09-15`, the entry headed *"KYO-702: BigQuery/Snowflake
  auth-mode-switch secret leak (strip_inactive_auth_mode_fields)"* — 2 🔴, unsigned. Finding
  #1: *"every save of a BigQuery(enterprise_oauth)/Snowflake(oauth)/Databricks(oauth)/
  Synapse(enterprise_oauth) datasource that doesn't retype the secret persists the literal
  placeholder over the real one — destroying the credential."* Finding #2 is the omission
  half, on `service_account_json`. The reviewer escalated both to CRITICAL despite their
  being pre-existing, because *"the diff's own new AC3-labeled test directly exercises this
  exact scenario and reports success while persisting a corrupted value."*
- **KYO-780** (PR #532, merged as `0f30cc5b`), review log `2026-09-15`, the two entries
  headed *"KYO-780: restore type-specific sensitive connection_config fields
  (oauth_client_secret / service_account_json)"* — the fix, plus the registry additions for
  Snowflake and Databricks. The re-review records why those additions are not scope creep:
  *"`oauth_auth_mode`'s `connection_config_fields` already declared both modes own this
  field, so it was returned in cleartext in every settings response for these two types — a
  live info-disclosure gap, independent of KYO-780's write-path bug."* The registry comment
  at `SNOWFLAKE_META` records the same reasoning in the source.
- **KYO-786** owns the remaining question these two fields raise — they are restored but
  still stored as plaintext, and encrypting them means fixing seven raw consumers first.
  Deferred deliberately, not overlooked.

Distinct from
[a helper that looks like a security control but has no caller](unused-security-helper-worse-than-none.md):
there the masking apparatus exists and nothing in production calls it, so the question is
whether the control runs at all. Here both sides run on every request and disagree about
which fields they cover. Distinct from
[propagate predicate changes to every copy](../code-organization/propagate-predicate-changes-to-every-copy.md):
that is N copies of one predicate with one left unedited; here the second site was never
taught to read the registry in the first place, so there was no copy to keep in sync.

See also
[an expected value read off the current behaviour guards the defect](../testing/an-expected-value-read-off-the-current-behaviour-guards-the-defect.md),
mined from this same review — the test that asserted `MASKED_VALUE` was the correct stored
value, and so argued against its own fix. That rule governs the assertion; this one governs
the code it was asserting. And
[tightening a column constraint requires auditing every write site](../data-state-management/audit-write-sites-when-tightening-constraint.md)
for the general shape: a property declared in one place, enforced at call sites nobody
enumerated.
