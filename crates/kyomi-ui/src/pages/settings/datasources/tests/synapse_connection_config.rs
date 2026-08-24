//! KYO-516 — every Azure Synapse datasource created through the Leptos UI
//! was permanently broken: `build_connection_config`'s `"synapse"` arm
//! wrote the endpoint under `"host"`, a verbatim copy-paste of the
//! `"sqlserver"` arm immediately above it, but the driver
//! (`kyomi-connect` `crates/kyomi-datasource/src/providers/synapse.rs`)
//! requires `"server"` and has no fallback — every Test & Discover and
//! every query failed with "Azure Synapse requires a server address".
//!
//! These tests pin: (1) the `"synapse"` arm of `build_connection_config`
//! emits `SYNAPSE_SERVER_CONFIG_KEY` ("server"), not "host"; (2) the key
//! name itself, as a named constant rather than a bare literal repeated
//! at every call site; (3) the edit-mode load-back reads the same key
//! back for Synapse rows; and (4) the `"sqlserver"` arm — the block this
//! was copy-pasted from — still emits `host`/`port`/`encrypt`/
//! `trust_server_certificate`, so a future cleanup of the Synapse arm
//! doesn't over-delete from the shared block by mistake.

use super::super::SYNAPSE_SERVER_CONFIG_KEY;
use super::{extract_between, SRC};

/// The full body of `build_connection_config`, scoped away from every
/// other `"synapse" => {` / `"sqlserver" => {` match arm in the file (the
/// edit-mode load-back, `build_credentials`, the connection-field view
/// fragments, the OAuth-status-fetch match, ...) so the narrower
/// `extract_between` calls below can't false-match one of those instead.
fn build_connection_config_body() -> &'static str {
    extract_between(
        SRC,
        "let build_connection_config = move || -> serde_json::Value {",
        "// ── Build credentials JSON",
    )
}

// ── KYO-516: the write side must use SYNAPSE_SERVER_CONFIG_KEY, not "host" ──

#[test]
fn synapse_arm_writes_server_not_host() {
    let body = build_connection_config_body();
    let synapse_arm = extract_between(body, "\"synapse\" => {", "\"bigquery\" => {");

    assert!(
        synapse_arm.contains("map.insert(SYNAPSE_SERVER_CONFIG_KEY.to_string()"),
        "build_connection_config's \"synapse\" arm must write the endpoint under \
         SYNAPSE_SERVER_CONFIG_KEY (\"server\") — the Synapse driver \
         (kyomi-connect providers/synapse.rs) requires exactly that key and has \
         no fallback, so any other key leaves every Synapse datasource \
         permanently broken with \"Azure Synapse requires a server address\" \
         (KYO-516)"
    );
    assert!(
        !synapse_arm.contains("map.insert(\"host\".to_string()"),
        "build_connection_config's \"synapse\" arm must not write \"host\" — that \
         is what the sqlserver arm it was copy-pasted from uses, and the Synapse \
         driver never reads it (KYO-516)"
    );
}

// ── Cross-file contract: pin the key name itself, not just where it's used ──

#[test]
fn synapse_server_config_key_matches_the_driver_contract() {
    // The Synapse driver lives in a different repo (kyomi-connect,
    // crates/kyomi-datasource/src/providers/synapse.rs) and cannot be
    // called, or even read, from a kyomi-ui test — so this test cannot
    // verify the driver's actual behavior. What it CAN do is pin the UI's
    // half of the contract to one named constant instead of a bare string
    // literal repeated at every call site, so a future edit that changes
    // SYNAPSE_SERVER_CONFIG_KEY's value (e.g. back to "host", or a typo)
    // has to also update — and think about — this assertion, rather than
    // silently drifting apart from the driver's expectation while the
    // build stays green.
    //
    // What this does NOT catch: the driver side changing independently —
    // a kyomi-connect release renaming the key the provider reads. That
    // mismatch can only be caught by an integration/E2E test that
    // actually exercises Test & Discover against a live Synapse
    // datasource, which is out of reach for a same-repo unit test.
    assert_eq!(
        SYNAPSE_SERVER_CONFIG_KEY, "server",
        "the Azure Synapse driver (kyomi-connect providers/synapse.rs) requires \
         connection_config[\"server\"] and has no fallback; if this ever needs \
         to change, it must change in lockstep with the driver (KYO-516)"
    );
}

// ── Round-trip: a `server`-bearing connection_config loads and saves without loss ──

#[test]
fn edit_mode_load_back_reads_server_for_synapse() {
    let load_back = extract_between(
        SRC,
        "let str_val = |key: &str| -> String {",
        "set_cfg_port.try_set(",
    );
    assert!(
        load_back.contains("if settings.datasource_type == \"synapse\""),
        "the edit-mode load-back must branch on datasource_type == \"synapse\" \
         before deciding which connection_config key to read for the endpoint \
         field — otherwise every datasource type falls through to str_val(\"host\") \
         and a Synapse row's server field loads blank on edit (KYO-516)"
    );
    assert!(
        load_back.contains("str_val(SYNAPSE_SERVER_CONFIG_KEY)"),
        "the synapse branch of the load-back must read SYNAPSE_SERVER_CONFIG_KEY \
         (\"server\"), matching what build_connection_config now writes — \
         otherwise editing a React-era Synapse row (which has a working `server` \
         value) loads a blank Server field and silently deletes it on save \
         (KYO-516)"
    );
}

// ── Regression guard: the sqlserver arm this was copied from must be untouched ──

#[test]
fn sqlserver_arm_still_writes_host_port_encrypt_and_trust_cert() {
    let body = build_connection_config_body();
    let sqlserver_arm = extract_between(body, "\"sqlserver\" => {", "\"synapse\" => {");

    for expected in [
        "map.insert(\"host\".to_string()",
        "map.insert(\"port\".to_string()",
        "map.insert(\"encrypt\".to_string()",
        "map.insert(\"trust_server_certificate\".to_string()",
    ] {
        assert!(
            sqlserver_arm.contains(expected),
            "sqlserver's build_connection_config arm must still write {expected} — \
             SQL Server genuinely uses host/port/encrypt/trust_server_certificate \
             (kyomi-connect providers/sqlserver.rs), unlike Synapse; the fix for \
             KYO-516 must not remove any of these from the sqlserver arm it was \
             copy-pasted from"
        );
    }
}
