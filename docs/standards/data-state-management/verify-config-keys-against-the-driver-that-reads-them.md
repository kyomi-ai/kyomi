# Verify a `connection_config` / `credentials` key against the driver that reads it

The datasource modal builds two untyped `serde_json::Value` maps — `connection_config`
and `credentials` — and the code that consumes them lives in a **different repository**
(`~/repos/kyomi-connect`, `crates/kyomi-datasource/src/providers/`). Nothing checks that
the two halves agree. There is no shared struct, no schema, and no test in either repo
that can fail: the UI writes the key it was written to write, the driver reads the key it
was written to read, and if those are not the same string the feature simply never works.
Both sides stay green forever, and the only symptom is a provider error a user sees at
connect time.

Three shapes of the same mismatch, all found in real diffs:

- **Wrong key name.** `build_connection_config`'s `"synapse"` arm was copy-pasted from
  `"sqlserver"` and wrote `host`. The driver does `connection_config.get("server")` with
  `.ok_or_else(|| Error::Provider("Azure Synapse requires a server address"))?` and no
  fallback. Every Synapse datasource ever created in the Leptos UI was permanently
  unusable, and nothing in this repo could have told you.
- **Right key, wrong map.** `tenant_id` was written into `connection_config`, but
  `get_service_principal_auth(credentials: &Value)` takes *only* `credentials` — it has
  no `connection_config` parameter, so there was no path by which it could ever have
  found the value. The field was filled in, and the driver still said it was missing.
- **Right key, wrong JSON type.** The reader is `.as_bool().unwrap_or(false)` or
  `.as_u64()`, so a leaf that arrives as `Value::String("true")` instead of `Value::Bool`
  reads as `false` — silently, on the default path, with no error anywhere.

The mirror image matters just as much: a field can look load-bearing and be inert, or
look inert and be load-bearing. Before *deleting* a config field, the driver is again the
only authority.

**Rule:** For every key you add, rename, remove, or re-map in `build_connection_config`
or `build_credentials`, open the provider in
`~/repos/kyomi-connect/crates/kyomi-datasource/src/providers/<provider>.rs` and read the
site that consumes it. Confirm three things: the exact key string; **which map** the
reader is handed (a function whose only parameter is `credentials` cannot see
`connection_config`, and vice versa); and the JSON type it extracts with. Then spell the
key once, as a named constant whose doc comment names the driver file and the error the
driver raises without it, so the write site and the edit-mode load-back both reference
it. Pin that constant with a test that states plainly what it *cannot* verify — a
same-repo test can hold the UI's half of the contract still, it can never observe the
driver's half.

```rust
// WRONG — key invented by copy-paste from a neighbouring arm; nothing in this
// repo disagrees, and the driver's `.get("server")` never matches.
"synapse" => {
    map.insert("host".to_string(), serde_json::json!(cfg_host.get_untracked()));
}

// RIGHT — one constant, doc-commented with the driver's actual requirement,
// referenced by both the write and the load-back.
/// The Azure Synapse driver (`kyomi-connect`
/// `crates/kyomi-datasource/src/providers/synapse.rs`) requires `"server"`
/// specifically and rejects `"host"` with
/// `Error::Provider("Azure Synapse requires a server address")`.
const SYNAPSE_SERVER_CONFIG_KEY: &str = "server";

"synapse" => {
    map.insert(
        SYNAPSE_SERVER_CONFIG_KEY.to_string(),
        serde_json::json!(cfg_host.get_untracked()),
    );
}
```

Four incidents, `2026-08-23` and `2026-08-24` review logs:

- **KYO-516** (`10:15`, 2026-08-24) — the `host`/`server` mismatch above. The review
  confirmed it by reading `providers/synapse.rs` directly rather than trusting the ticket:
  `server` hard-required, `port` and `encrypt` hardcoded in the driver (so the UI's copies
  were inert), and `trust_server_certificate` genuinely read via `config.trust_cert()` — so
  removing that third field was a real behaviour change needing its own justification, not
  dead-code cleanup. Shipped as `SYNAPSE_SERVER_CONFIG_KEY` plus
  `synapse_server_config_key_matches_the_driver_contract`, whose doc comment says outright
  that it cannot see the driver.
- **KYO-522** (`08:15`, 2026-08-24) — the `tenant_id` wrong-map case, found *as a discovery
  during KYO-516's own review* and filed rather than fixed inline. Fix duplicates the write
  into `build_credentials`'s `"service_principal"` arm; the `connection_config` write stays,
  because `enterprise_oauth` still needs it there.
- **KYO-415** (`06:23`, 2026-08-24) — the deletion case done right. Removing BigQuery's
  Default Project field was safe only because
  `grep -rn "default_project" ~/repos/kyomi-connect/crates/` returned nothing and
  `resolve_billing_project` provably reads only `billing_project`/`default_billing_project`.
  The grep across the sibling repo *was* the evidence.
- **KYO-460** (`14:00`, 2026-08-23) — the wrong-type case. A migration retyped the corrupted
  boolean leaves in `connection_config`, but the PR's "six is the complete set" was
  empirically false: `shared_credentials`, read by `resolve_shared_credentials`
  (`~/repos/kyomi-connect/crates/kyomi-datasource/src/factory.rs:82`) via
  `.as_bool().unwrap_or(false)`, was a seventh.

Sibling of
[audit-write-sites-when-tightening-constraint.md](audit-write-sites-when-tightening-constraint.md):
that rule is about a *column* whose constraint tightened under existing Rust writers. This
one is about an *untyped JSON leaf* whose reader is in another repo, where there is no
constraint to tighten and no failure until a user tries to connect. See also
[enumerate-consumers-from-the-type-not-from-the-diff.md](../code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md),
which covers the sweep when a producer *changes*; this rule applies equally to a key that
was wrong on the day it was written and never changed at all.
