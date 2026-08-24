//! KYO-522 — Azure Synapse Service Principal auth could never succeed:
//! the driver (`kyomi-connect` `crates/kyomi-datasource/src/providers/synapse.rs`
//! `get_service_principal_auth`, lines ~283-296) reads `tenant_id`,
//! `client_id` and `client_secret` all three from `credentials`. The
//! `"service_principal"` arm of `build_credentials` wrote `client_id` and
//! `client_secret` there correctly, but `tenant_id` was written only into
//! `connection_config` by `build_connection_config` — so every Service
//! Principal connection failed with "Service Principal requires
//! tenant_id" even with the field filled in on screen. This is the
//! sibling of KYO-516 (`host` vs `server`): a key the UI writes into one
//! bag, the driver reads from another. KYO-516's fix (#414) is what
//! exposed this — before it, every Synapse datasource failed earlier on
//! the missing `server`, so this arm was never reached.
//!
//! The fix is a second write, not a move: `connection_config["tenant_id"]`
//! must stay, because `enterprise_oauth` mode needs the same tenant value
//! there (see `build_connection_config`'s `"synapse"` arm). These tests
//! pin: (1) the `"service_principal"` arm of `build_credentials` now
//! writes `tenant_id`; (2) the `"synapse"` arm of `build_connection_config`
//! still writes it too, so a future "cleanup" that deletes the
//! connection_config copy (mistaking it for now-redundant) doesn't
//! silently break `enterprise_oauth`; (3) the credentials-side write reads
//! `cfg_tenant_id` — the same signal the edit-mode load-back populates
//! from `connection_config` — so a saved-then-reopened-then-resaved
//! Service Principal datasource keeps both copies, rather than the
//! credentials copy going stale or empty on edit.

use super::{extract_between, SRC};

/// The full body of `build_credentials`, scoped away from
/// `build_connection_config` above it and `test_action` (Test & Discover)
/// below it, so the narrower `extract_between` calls below can't
/// false-match a `"synapse" => {` or `"service_principal" => {` arm
/// belonging to a different closure or view fragment.
fn build_credentials_body() -> &'static str {
    extract_between(
        SRC,
        "let build_credentials = move || -> serde_json::Value {",
        "let test_action = Action::new(|input: &TestDiscoverInput| {",
    )
}

fn credentials_synapse_arm() -> &'static str {
    let body = build_credentials_body();
    extract_between(body, "\"synapse\" => {", "_ => {")
}

fn credentials_service_principal_arm() -> &'static str {
    let synapse_arm = credentials_synapse_arm();
    extract_between(synapse_arm, "\"service_principal\" => {", "\"enterprise_oauth\" => {")
}

// ── KYO-522: the credentials-side write must exist ──

#[test]
fn service_principal_credentials_arm_writes_tenant_id() {
    let sp_arm = credentials_service_principal_arm();

    assert!(
        sp_arm.contains("map.insert(\"tenant_id\".to_string()"),
        "build_credentials's \"service_principal\" arm must write tenant_id — the \
         Synapse driver's get_service_principal_auth (kyomi-connect \
         providers/synapse.rs) reads credentials[\"tenant_id\"] directly and has \
         no fallback to connection_config, so omitting it here fails every \
         Service Principal connection with \"Service Principal requires \
         tenant_id\" even when the field is filled in on screen (KYO-522)"
    );
}

#[test]
fn service_principal_credentials_tenant_id_uses_the_shared_cfg_signal() {
    let sp_arm = credentials_service_principal_arm();

    assert!(
        sp_arm.contains("cfg_tenant_id.get_untracked()"),
        "the tenant_id write in build_credentials's \"service_principal\" arm \
         must read cfg_tenant_id — the same signal build_connection_config reads \
         and the edit-mode load-back populates from connection_config[\"tenant_id\"] \
         — otherwise editing a saved Service Principal datasource and re-saving \
         it would need a second, unsynced source of truth for the same field \
         (KYO-522)"
    );
}

// ── Regression guard: connection_config's copy must survive ──
// enterprise_oauth mode reads tenant_id from connection_config (there is no
// per-user OAuth credential for it), so the fix must add a second write, not
// relocate the existing one.

#[test]
fn connection_config_still_writes_tenant_id_for_synapse() {
    let build_connection_config_body = extract_between(
        SRC,
        "let build_connection_config = move || -> serde_json::Value {",
        "// ── Build credentials JSON",
    );
    let synapse_arm =
        extract_between(build_connection_config_body, "\"synapse\" => {", "\"bigquery\" => {");

    assert!(
        synapse_arm.contains("map.insert(\"tenant_id\".to_string()"),
        "build_connection_config's \"synapse\" arm must still write tenant_id — \
         enterprise_oauth mode reads it from connection_config (there is no \
         per-user credential for the tenant), so KYO-522's fix must add a second \
         write to credentials for service_principal mode, not move the existing \
         connection_config write"
    );
}
